//! Read-only data for the Backbone views a pane can show.
//!
//! Each [`Request`] is a few GETs of fixed local API paths, with the same
//! address, credential, proxy and redirect rules as the roster. Decoding keeps
//! only the fields a view draws; an item that does not decode is skipped.
//! Team-wide libraries are gathered in the private `library` module.

use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{configuration::configured, transport, valid_agent, within_depth};

mod library;

pub use library::{
    Loadout, LocalSkill, PREVIEW_LIMIT, Place, PolicyLibrary, Reason, SkillLibrary, TagCard,
    TagLibrary,
};

const VIEW_LIMIT: usize = 4 * 1024 * 1024;

/// Why a view has no data. The text is fixed and contains no response content.
pub const UNAVAILABLE: &str = "Backbone is not answering.";
/// Backbone answered that it does not serve this read.
pub const NOT_SERVED: &str = "This Backbone does not serve policies yet; it needs updating.";
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
    /// Every skill an agent can load, team-wide.
    Skills,
    /// The layers a brief is built from, team-wide.
    Policies,
    /// What each tag brings.
    Tags,
    /// One agent's card, with the part its shown section needs.
    Agent(String, Part),
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
    /// Its usage over a range.
    Usage(Range),
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
    /// See [`SkillLibrary`].
    Skills(Box<SkillLibrary>),
    /// See [`PolicyLibrary`].
    Policies(Box<PolicyLibrary>),
    /// See [`TagLibrary`].
    Tags(Box<TagLibrary>),
    /// See [`Agent`].
    Agent(Box<Agent>),
}

/// An enabled provider's address and key, and whether the read is still
/// wanted.
pub(crate) struct Api<'a> {
    url: String,
    key: String,
    live: &'a dyn Fn() -> bool,
}

impl<'a> Api<'a> {
    #[cfg(test)]
    pub(crate) fn new(url: String, key: String, live: &'a dyn Fn() -> bool) -> Self {
        Self { url, key, live }
    }

    fn live(&self) -> bool {
        (self.live)()
    }

    fn get(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        // A read nobody is waiting for any more starts no further request.
        if !self.live() {
            return Err(UNAVAILABLE);
        }
        transport::get(&self.url, path, &self.key, VIEW_LIMIT).ok_or(UNAVAILABLE)
    }
}

/// What one agent receives at its next start.
fn instructions(api: &Api<'_>, name: &str) -> Result<Instructions, &'static str> {
    if !api.live() {
        return Err(UNAVAILABLE);
    }
    let path = format!("/api/agents/{name}/instructions");
    match transport::fetch(&api.url, &path, &api.key, VIEW_LIMIT) {
        Ok(body) => Instructions::decode(&body),
        Err(Some(404 | 405)) => Err(NOT_SERVED),
        Err(_) => Err(UNAVAILABLE),
    }
}

fn usage_path(range: Range) -> String {
    let since = iso(now().saturating_sub(range.seconds()));
    format!("/api/usage?refresh=false&limit=1000&since={since}")
}

/// Read one request from an enabled provider. Blocks for up to three seconds
/// per GET, so call it on an I/O worker. Configuration is re-read every time.
/// A library read stops starting per-agent requests once `live` says no one
/// is waiting for it any more.
pub fn read(
    data_dir: &Path,
    url: Option<&str>,
    request: &Request,
    live: &dyn Fn() -> bool,
) -> Result<Data, &'static str> {
    let (url, key) = configured(data_dir, url).ok_or(UNAVAILABLE)?;
    let api = Api { url, key, live };
    match request {
        Request::Team => Ok(Data::Team(Team::decode(
            &api.get("/api/agents")?,
            &api.get("/api/swarms")?,
        )?)),
        Request::Usage(range) => Ok(Data::Usage(Usage::decode(&api.get(&usage_path(*range))?)?)),
        Request::Activity => {
            // History is an addition: when it cannot be read, the timeline
            // still shows deliveries and states now, and says why.
            let changes = history(&api, now().saturating_sub(3_600 + HISTORY_LOOKBACK));
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
        Request::Skills => Ok(Data::Skills(Box::new(SkillLibrary::read(&api)?))),
        Request::Policies => {
            let agents = library::agents(&api)?;
            Ok(Data::Policies(Box::new(PolicyLibrary::read(
                &api, &agents,
            )?)))
        }
        Request::Tags => Ok(Data::Tags(Box::new(TagLibrary::read(&api)?))),
        Request::Agent(name, part) => {
            if !valid_agent(name) {
                return Err(UNREADABLE);
            }
            let path = format!("/api/agents/{name}/inspect");
            let mut agent = Agent::decode(&api.get(&path)?)?;
            agent.launch = items_of::<Launch>(&api.get("/api/config/agents")?)
                .into_iter()
                .find(|launch| &launch.name == name);
            agent.permissions = read_permissions(&agent.runtime, &agent.dir);
            // A section's own read failing leaves the card; the section says why.
            match part {
                Part::Card => {}
                Part::Skills => {
                    agent.skills = Some(
                        api.get(&format!("/api/skills/preview/{name}"))
                            .and_then(|body| Loadout::decode(&body)),
                    );
                }
                Part::Policies => agent.brief = Some(instructions(&api, name)),
                Part::Usage(range) => {
                    agent.usage = Some(
                        api.get(&usage_path(*range))
                            .and_then(|body| Usage::decode(&body))
                            .map(|usage| usage.of(name)),
                    );
                }
            }
            Ok(Data::Agent(Box::new(agent)))
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
    /// Its registered workspace folder, or empty.
    pub dir: String,
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
#[derive(Clone, Debug, Default, PartialEq)]
pub enum History {
    /// This Backbone keeps no state history.
    #[default]
    NotServed,
    /// Backbone keeps it, but it could not be read this time.
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
        let body = match transport::fetch(&api.url, &path, &api.key, VIEW_LIMIT) {
            Ok(body) => body,
            Err(Some(404 | 405)) => return History::NotServed,
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
    /// Skill name.
    pub name: String,
    /// Its folder in the store, as Backbone reports it.
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
    /// Sources it is subscribed to.
    pub subscriptions: Vec<Subscription>,
    /// Its most recent deliveries.
    #[serde(skip)]
    pub received: Vec<Received>,
    /// How Backbone launches it, when Backbone configures it.
    #[serde(skip)]
    pub launch: Option<Launch>,
    /// What its CLI lets it do without asking.
    #[serde(skip)]
    pub permissions: Permissions,
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

/// How Backbone launches an agent, from its configuration.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Launch {
    /// Agent name.
    pub name: String,
    /// Started without permission prompts.
    pub unattended: bool,
    /// Kept running.
    pub always_on: bool,
    /// Receives messages only through its inbox.
    pub inbox_only: bool,
}

/// One settings file a CLI reads its rules from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleFile {
    /// Its path.
    pub path: String,
    /// Whether it exists and was read.
    pub read: bool,
}

/// What an agent's CLI lets it do without asking, from the CLI's own
/// settings files. Only the files named here are read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Permissions {
    /// The files consulted, in the CLI's order.
    pub files: Vec<RuleFile>,
    /// Actions allowed without asking.
    pub allow: Vec<String>,
    /// Actions always refused.
    pub deny: Vec<String>,
    /// Actions that always ask.
    pub ask: Vec<String>,
    /// Other settings that decide approval, as name and value.
    pub modes: Vec<(String, String)>,
}

fn read_bounded(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// Read the approval rules of `runtime` for an agent working in `dir`.
/// Claude Code: `~/.claude/settings.json`, then the directory's
/// `.claude/settings.json` and `.claude/settings.local.json`. Codex:
/// `~/.codex/config.toml`. Other CLIs: nothing.
pub fn read_permissions(runtime: &str, dir: &str) -> Permissions {
    rules(
        runtime,
        dir,
        std::env::var_os("HOME").map(std::path::PathBuf::from),
    )
}

fn rules(runtime: &str, dir: &str, home: Option<std::path::PathBuf>) -> Permissions {
    let mut permissions = Permissions::default();
    let add = |path: std::path::PathBuf, permissions: &mut Permissions| {
        let text = read_bounded(&path);
        permissions.files.push(RuleFile {
            path: path.display().to_string(),
            read: text.is_some(),
        });
        text
    };
    match runtime {
        "claude" => {
            let mut files = Vec::new();
            files.extend(home.map(|home| home.join(".claude/settings.json")));
            if Path::new(dir).is_absolute() {
                files.push(Path::new(dir).join(".claude/settings.json"));
                files.push(Path::new(dir).join(".claude/settings.local.json"));
            }
            for path in files {
                #[derive(Deserialize, Default)]
                #[serde(default, rename_all = "camelCase")]
                struct Rules {
                    allow: Vec<String>,
                    deny: Vec<String>,
                    ask: Vec<String>,
                    default_mode: Option<String>,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Settings {
                    permissions: Rules,
                }
                let Some(text) = add(path, &mut permissions) else {
                    continue;
                };
                let rules = serde_json::from_str::<Settings>(&text)
                    .unwrap_or_default()
                    .permissions;
                for (into, from) in [
                    (&mut permissions.allow, rules.allow),
                    (&mut permissions.deny, rules.deny),
                    (&mut permissions.ask, rules.ask),
                ] {
                    for rule in from {
                        if !into.contains(&rule) {
                            into.push(rule);
                        }
                    }
                }
                if let Some(mode) = rules.default_mode {
                    permissions.modes.retain(|(name, _)| name != "defaultMode");
                    permissions.modes.push(("defaultMode".into(), mode));
                }
            }
        }
        "codex" => {
            if let Some(home) = home
                && let Some(text) = add(home.join(".codex/config.toml"), &mut permissions)
            {
                // Only the top-level approval settings.
                let table = text.parse::<toml::Table>().unwrap_or_default();
                for name in ["approval_policy", "sandbox_mode"] {
                    if let Some(value) = table.get(name).and_then(toml::Value::as_str) {
                        permissions.modes.push((name.into(), value.into()));
                    }
                }
            }
        }
        _ => {}
    }
    permissions
}

/// One source of an agent's startup instructions.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Source {
    /// The policy's name, or `base` or `swarm`.
    pub name: String,
    /// Its file.
    pub path: String,
    /// Why the agent gets it: `base`, `global`, `tag:…` or `swarm`.
    pub scope: String,
    /// Whether it is applied at the next start.
    pub applied: bool,
    /// The file's text, when Backbone could read it.
    pub text: Option<String>,
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
    pub sources: Vec<Source>,
    /// Backbone's notes about this preview.
    pub notices: Vec<String>,
}

impl Instructions {
    fn decode(body: &[u8]) -> Result<Self, &'static str> {
        document(body)?;
        serde_json::from_slice(body).map_err(|_| UNREADABLE)
    }
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

/// The largest part of a SKILL.md that is read.
pub const SKILL_LIMIT: usize = 256 * 1024;

/// Read the SKILL.md in a skill folder Backbone reported, up to
/// [`SKILL_LIMIT`] bytes ending at a whole character. Returns the text and
/// whether it was cut. Nothing outside that one file is read. Blocking.
pub fn read_skill(folder: &str) -> Result<(String, bool), &'static str> {
    use std::io::Read;
    const MISSING: &str = "This skill's SKILL.md cannot be read.";
    let path = Path::new(folder);
    if !path.is_absolute() {
        return Err(MISSING);
    }
    let file = std::fs::File::open(path.join("SKILL.md")).map_err(|_| MISSING)?;
    let mut bytes = Vec::new();
    file.take(SKILL_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| MISSING)?;
    let cut = bytes.len() > SKILL_LIMIT;
    bytes.truncate(SKILL_LIMIT);
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let valid = error.utf8_error().valid_up_to();
            if !cut || error.utf8_error().error_len().is_some() {
                return Err(MISSING);
            }
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).map_err(|_| MISSING)?
        }
    };
    Ok((text, cut))
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

// A top-level array, skipping entries that do not decode.
fn items_of<T: DeserializeOwned>(body: &[u8]) -> Vec<T> {
    if body.len() > VIEW_LIMIT || !within_depth(body) {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<&RawValue>>(body)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| serde_json::from_str(item.get()).ok())
        .collect()
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
    fn skill_text_is_bounded_at_a_whole_character() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().to_str().unwrap();
        std::fs::write(folder.path().join("SKILL.md"), "# Check\n").unwrap();
        assert_eq!(read_skill(path), Ok(("# Check\n".into(), false)));
        let long = format!("{}é", "a".repeat(SKILL_LIMIT - 1));
        std::fs::write(folder.path().join("SKILL.md"), long).unwrap();
        let (text, cut) = read_skill(path).unwrap();
        assert!(cut && text.len() == SKILL_LIMIT - 1);
        assert!(read_skill("relative").is_err());
    }

    #[test]
    fn claude_rules_merge_across_files_and_policies_decode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(git *)"],"deny":["Read(.env)"],"defaultMode":"acceptEdits"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.local.json"),
            r#"{"permissions":{"allow":["Bash(git *)","Bash(make *)"]}}"#,
        )
        .unwrap();
        let rules = rules("claude", dir.path().to_str().unwrap(), None);
        assert_eq!(rules.allow, ["Bash(git *)", "Bash(make *)"]);
        assert_eq!(rules.deny, ["Read(.env)"]);
        assert_eq!(
            rules.modes,
            [("defaultMode".to_owned(), "acceptEdits".to_owned())]
        );
        assert_eq!(rules.files.iter().filter(|file| file.read).count(), 2);
        assert_eq!(read_permissions("other", "/").files.len(), 0);
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        std::fs::write(
            home.path().join(".codex/config.toml"),
            "approval_policy = 'never' # quiet\n[profiles.x]\nsandbox_mode = \"read-only\"\n",
        )
        .unwrap();
        let codex = super::rules("codex", "/", Some(home.path().to_owned()));
        assert_eq!(
            codex.modes,
            [("approval_policy".to_owned(), "never".to_owned())]
        );

        let brief = Instructions::decode(
            br##"{"name":"worker","runtime":"claude","mode":"system","enabled":true,
            "sources":[{"name":"base","path":"/t/base.md","scope":"base","applied":true,"text":"# Base"},
            {"name":"review","path":"/t/review.md","scope":"tag:review","applied":true,"text":null}],
            "content":"...","notices":["note"]}"##,
        )
        .unwrap();
        assert_eq!(brief.sources[1].scope, "tag:review");
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
        let activity = Activity::decode(deliveries, events, agents, History::NotServed).unwrap();
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
    }
}
