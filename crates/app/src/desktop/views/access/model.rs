//! What the Access pane shows, worked out from what Backbone served: agents
//! grouped by role and swarm, each row's words, the filters' counts, the
//! chain of agents above one, and the requests split by who they wait on.
//! Replies to changes are applied here as Backbone returned them.
use agent_interface_roster::{
    Swarm,
    profiles::{
        access::{
            Access, Agent, AgentAccess, Grant, Mode, Overview, Placed, Reach, Request,
            RequestState, Role, Source,
        },
        browsers::{Browser, Browsers, Session},
    },
};
use chrono::{DateTime, FixedOffset};

/// The served records the pane draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Model {
    pub overview: Overview,
    /// Every waiting request, the newest first.
    pub pending: Vec<Request>,
    /// Decided requests read so far, the newest first; none until read.
    pub decided: Option<Vec<Request>>,
    /// The named browsers, as last read or changed.
    pub browsers: Browsers,
    /// Each browser's newest session, by its ID, read while an agent's
    /// detail shows the browsers; a browser never used has none.
    pub newest: std::collections::HashMap<String, Session>,
}

/// The agent list's groups, in the board's order.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Groups<'a> {
    pub orchestrators: Vec<&'a Agent>,
    pub coordinators: Vec<&'a Agent>,
    pub main: Vec<&'a Agent>,
    pub swarms: Vec<SwarmGroup<'a>>,
}

/// One swarm's row, collapsed to its members.
#[derive(Debug, PartialEq)]
pub(super) struct SwarmGroup<'a> {
    pub name: String,
    /// The agent that started it, when the swarms read says.
    pub initiator: Option<String>,
    pub members: Vec<&'a Agent>,
}

impl Groups<'_> {
    /// How many agents the swarms hold.
    pub fn swarm_agents(&self) -> usize {
        self.swarms.iter().map(|swarm| swarm.members.len()).sum()
    }
}

/// The list's filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    /// Orchestrators and coordinators.
    AboveOthers,
    /// Agents granted the browser.
    Browser,
    Unsandboxed,
}

impl Filter {
    pub const ALL: [Self; 4] = [
        Self::All,
        Self::AboveOthers,
        Self::Browser,
        Self::Unsandboxed,
    ];

    pub fn matches(self, agent: &Agent) -> bool {
        match self {
            Self::All => true,
            Self::AboveOthers => matches!(agent.role, Some(Role::Orchestrator | Role::Coordinator)),
            Self::Browser => browsers(agent) > 0,
            Self::Unsandboxed => agent.mode == Mode::Unsandboxed,
        }
    }

    /// How many of `agents` it keeps.
    pub fn count(self, agents: &[Agent]) -> usize {
        agents.iter().filter(|agent| self.matches(agent)).count()
    }
}

/// The swarm `agent` works in: its `swarm:` tag, or the swarm listing it.
fn swarm_of<'a>(agent: &'a Agent, swarms: &'a [Swarm]) -> Option<&'a str> {
    agent
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix("swarm:"))
        .filter(|name| !name.is_empty())
        .or_else(|| {
            swarms
                .iter()
                .find(|swarm| swarm.members.contains(&agent.name))
                .map(|swarm| swarm.name.as_str())
        })
}

/// Group the agents `filter` keeps, by role; swarm agents by their swarm, in
/// the order each swarm is first met. A swarm agent whose swarm is not known,
/// or an agent of a role this version does not know, is listed with the main
/// agents, so no agent is hidden.
pub(super) fn groups(overview: &Overview, filter: Filter) -> Groups<'_> {
    matching(overview, filter, "")
}

/// As [`groups`], keeping only agents whose name holds `query`, ignoring
/// case.
pub(super) fn matching<'a>(overview: &'a Overview, filter: Filter, query: &str) -> Groups<'a> {
    let query = query.trim().to_lowercase();
    let mut groups = Groups::default();
    for agent in overview
        .agents
        .iter()
        .filter(|agent| filter.matches(agent) && agent.name.to_lowercase().contains(&query))
    {
        match agent.role {
            Some(Role::Orchestrator) => groups.orchestrators.push(agent),
            Some(Role::Coordinator) => groups.coordinators.push(agent),
            Some(Role::SwarmCoordinator | Role::SwarmMember) => {
                match swarm_of(agent, &overview.swarms) {
                    Some(name) => match groups.swarms.iter_mut().find(|s| s.name == name) {
                        Some(swarm) => swarm.members.push(agent),
                        None => groups.swarms.push(SwarmGroup {
                            name: name.to_owned(),
                            initiator: overview
                                .swarms
                                .iter()
                                .find(|swarm| swarm.name == name)
                                .map(|swarm| swarm.initiator.clone()),
                            members: vec![agent],
                        }),
                    },
                    None => groups.main.push(agent),
                }
            }
            _ => groups.main.push(agent),
        }
    }
    groups
}

/// Who a row says the agent answers to.
pub(super) fn answers_to(agent: &Agent) -> String {
    match &agent.superior {
        None => "answers to you".into(),
        Some(superior) => format!("under {superior}"),
    }
}

/// How many repositories a row says the agent reaches.
pub(super) fn reach_label(reach: &Reach) -> String {
    let items = match reach {
        Reach::Profile { .. } => return "every repository".into(),
        Reach::Repositories { items } => items.len(),
        Reach::Unknown => return "repositories not known".into(),
    };
    match items {
        0 => "no repository".into(),
        1 => "1 repository".into(),
        n => format!("{n} repositories"),
    }
}

/// How many browser grants the agent holds.
pub(super) fn browsers(agent: &Agent) -> usize {
    agent
        .grants
        .iter()
        .filter(|grant| matches!(grant, Grant::Browser { .. }))
        .count()
}

/// How many secrets, variables and files, the agent holds.
pub(super) fn secrets(agent: &Agent) -> usize {
    agent
        .grants
        .iter()
        .filter(|grant| {
            matches!(
                grant,
                Grant::SecretVariable { .. } | Grant::SecretFile { .. }
            )
        })
        .count()
}

/// The role's name as the pane writes it.
pub(super) fn role_label(role: Role) -> &'static str {
    match role {
        Role::Orchestrator => "Orchestrator",
        Role::Coordinator => "Coordinator",
        Role::Main => "Main agent",
        Role::SwarmCoordinator => "Swarm coordinator",
        Role::SwarmMember => "Swarm member",
        Role::Unknown => "Role not known",
    }
}

/// The agents above `name`, nearest first, up to the one directly under the
/// owner. It stops before an agent met twice, so a cycle in the records
/// ends, and at a superior that is not listed, which it still names.
pub(super) fn chain(agents: &[Agent], name: &str) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut at = agents.iter().find(|agent| agent.name == name);
    while let Some(superior) = at.and_then(|agent| agent.superior.as_ref()) {
        if superior == name || chain.contains(superior) {
            break;
        }
        chain.push(superior.clone());
        at = agents.iter().find(|agent| &agent.name == superior);
    }
    chain
}

/// How many waiting requests `name` made.
pub(super) fn asks(pending: &[Request], name: &str) -> usize {
    pending
        .iter()
        .filter(|request| request.agent == name)
        .count()
}

/// Waiting requests split into those on the owner and those on an agent,
/// each in the served order.
pub(super) fn waiting(pending: &[Request]) -> (Vec<&Request>, Vec<&Request>) {
    pending
        .iter()
        .filter(|request| request.state == RequestState::Pending)
        .partition(|request| request.waits_on.is_none())
}

/// "N requests from M agents".
pub(super) fn from_agents(requests: &[&Request]) -> String {
    let mut agents: Vec<&str> = requests.iter().map(|r| r.agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    let (n, m) = (requests.len(), agents.len());
    format!(
        "{n} request{} from {m} agent{}",
        if n == 1 { "" } else { "s" },
        if m == 1 { "" } else { "s" }
    )
}

pub(super) fn time(at: &str, now: &DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&now.timezone()))
}

/// A wait, in its largest whole unit.
pub(super) fn age(at: &DateTime<FixedOffset>, now: &DateTime<FixedOffset>) -> String {
    let minutes = (*now - *at).num_minutes().max(0);
    let (n, unit) = match minutes {
        0..60 => (minutes, "minute"),
        60..1440 => (minutes / 60, "hour"),
        _ => (minutes / 1440, "day"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// "The oldest has waited 8 days.", from the oldest waiting request; none
/// when nothing waits or no time can be read.
pub(super) fn oldest_wait(pending: &[Request], now: &DateTime<FixedOffset>) -> Option<String> {
    let oldest = pending
        .iter()
        .filter(|request| request.state == RequestState::Pending)
        .filter_map(|request| time(&request.requested_at, now))
        .min()?;
    Some(format!("The oldest has waited {}.", age(&oldest, now)))
}

/// When a request was made: "asked 12 minutes ago" within a day, then
/// "asked 18 Nov, 8 days ago"; with `date_only`, "asked 25 Nov".
pub(super) fn asked(request: &Request, now: &DateTime<FixedOffset>, date_only: bool) -> String {
    let Some(at) = time(&request.requested_at, now) else {
        return "asked at a time not known".into();
    };
    let date = at.format("%-d %b");
    if date_only {
        return format!("asked {date}");
    }
    if *now - at < chrono::Duration::days(1) {
        format!("asked {} ago", age(&at, now))
    } else {
        format!("asked {date}, {} ago", age(&at, now))
    }
}

/// How a decided request ended and who decided it: "approved by you",
/// "refused by planner".
pub(super) fn decided_label(request: &Request) -> String {
    let state = match request.state {
        RequestState::Approved => "approved",
        RequestState::Refused => "refused",
        RequestState::Pending => "waiting",
        RequestState::Unknown => "decided",
    };
    match (&request.decided_by, request.state) {
        (_, RequestState::Pending) => state.into(),
        (None, _) => format!("{state} by you"),
        (Some(by), _) => format!("{state} by {by}"),
    }
}

/// When a decided request was decided: "21 Nov 16:02".
pub(super) fn decided_at(request: &Request, now: &DateTime<FixedOffset>) -> Option<String> {
    let at = time(request.decided_at.as_deref()?, now)?;
    Some(at.format("%-d %b %H:%M").to_string())
}

/// The kind of thing a grant is, with the access it asks for and the
/// machine it is on, so one card holds an agent's requests of one kind and
/// one access on one machine.
/// What makes requests alike: their kind, access and machine, and for a
/// browser which one.
type Kind<'a> = (
    std::mem::Discriminant<Grant>,
    Option<Access>,
    Option<&'a str>,
    Option<&'a str>,
);

fn kind(grant: &Grant) -> Kind<'_> {
    let (access, browser) = match grant {
        Grant::Repository { access, .. } | Grant::Folder { access, .. } => (Some(*access), None),
        Grant::Browser { browser, .. } => (None, browser.as_deref()),
        _ => (None, None),
    };
    (
        std::mem::discriminant(grant),
        access,
        machine_id(grant),
        browser,
    )
}

/// What a grant's machine chip says: `this` for this app's own machine
/// (`local`, or none named), the name Backbone serves for another, or
/// "another machine" once Backbone names it no longer. Machine IDs are
/// never shown.
pub(super) fn machine_label(
    this: &str,
    local: &str,
    id: Option<&str>,
    name: Option<&str>,
) -> String {
    match (id, name) {
        (Some(id), _) if id == local => this.into(),
        (None, None) => this.into(),
        (_, Some(name)) => name.into(),
        (Some(_), None) => "another machine".into(),
    }
}

/// The machine a grant is on, by ID; none for a repository.
pub(super) fn machine_id(grant: &Grant) -> Option<&str> {
    match grant {
        Grant::Folder { machine, .. }
        | Grant::SecretFile { machine, .. }
        | Grant::SecretVariable { machine, .. }
        | Grant::Browser { machine, .. }
        | Grant::Docker { machine, .. } => machine.as_deref(),
        Grant::Repository { .. } | Grant::Unknown => None,
    }
}

/// Waiting requests as the board draws them: one card for an agent's
/// requests of the same kind and access that wait on the same decider, in
/// the order the first of them is met.
pub(super) fn cards<'a>(requests: &[&'a Request]) -> Vec<Vec<&'a Request>> {
    let mut cards: Vec<Vec<&Request>> = Vec::new();
    for request in requests {
        let same = |card: &&mut Vec<&Request>| {
            let first = card[0];
            first.agent == request.agent
                && first.waits_on == request.waits_on
                && kind(&first.grant) == kind(&request.grant)
                && request.grant != Grant::Unknown
        };
        match cards.iter_mut().find(|card| same(card)) {
            Some(card) => card.push(request),
            None => cards.push(vec![request]),
        }
    }
    cards
}

/// What a grant names: a repository, a path, a variable; none for the
/// browser, Docker or a kind this version does not know.
pub(super) fn thing(grant: &Grant) -> Option<String> {
    match grant {
        Grant::Repository { repo, .. } => Some(repo.clone()),
        Grant::Folder { path, .. } | Grant::SecretFile { path, .. } => Some(path.clone()),
        Grant::SecretVariable { name, .. } => Some(name.clone()),
        Grant::Browser { browser, .. } => browser.clone(),
        Grant::Docker { .. } | Grant::Unknown => None,
    }
}

/// The machine a grant is on, by name, when the reply names one.
pub(super) fn machine_of(grant: &Grant) -> Option<String> {
    match grant {
        Grant::Folder { machine_name, .. }
        | Grant::SecretFile { machine_name, .. }
        | Grant::SecretVariable { machine_name, .. }
        | Grant::Browser { machine_name, .. }
        | Grant::Docker { machine_name, .. } => machine_name.clone(),
        Grant::Repository { .. } | Grant::Unknown => None,
    }
}

/// What a card says the agent asks, and the thing it asks for when the card
/// holds one request: "asks to read" `example/app`, "asks to read 2
/// repositories", "asks to use" `browser`.
pub(super) fn phrase(card: &[&Request]) -> (String, Option<String>) {
    let Some(first) = card.first() else {
        return (String::new(), None);
    };
    let n = card.len();
    let plural = |one: &str, many: &str| {
        if n == 1 {
            one.to_owned()
        } else {
            format!("{n} {many}")
        }
    };
    let reads = |access: &Access| match access {
        Access::Read => "asks to read",
        Access::Write => "asks to write",
    };
    let (verb, many) = match &first.grant {
        Grant::Repository { access, .. } => (reads(access), plural("", "repositories")),
        Grant::Folder { access, .. } => (reads(access), plural("", "folders")),
        Grant::SecretVariable { .. } => ("asks for the secret", plural("", "secrets")),
        Grant::SecretFile { .. } => ("asks for the secret file", plural("", "secret files")),
        Grant::Browser { browser, .. } => {
            let named = browser.clone().unwrap_or_else(|| "browser".into());
            return ("asks to use".into(), Some(named));
        }
        Grant::Docker { .. } => return ("asks to use".into(), Some("Docker".into())),
        Grant::Unknown => return ("asks for access this version cannot show".into(), None),
    };
    if n == 1 {
        (verb.into(), thing(&first.grant))
    } else {
        // "asks for the secret" reads "asks for 2 secrets" when there are more.
        let verb = verb
            .trim_end_matches(" the secret file")
            .trim_end_matches(" the secret");
        (format!("{verb} {many}"), None)
    }
}

/// Why approving a card changes nothing, when Backbone's records say so:
/// the agent reads every repository, or already reaches what it asks for.
pub(super) fn not_needed(agent: &Agent, card: &[&Request]) -> Option<String> {
    let mut asked = Vec::new();
    for request in card {
        let Grant::Repository { repo, access } = &request.grant else {
            return None;
        };
        asked.push((repo, *access));
    }
    let reaches = |repo: &str, access: Access| -> bool {
        let items = match &agent.reach {
            Reach::Profile { .. } if access == Access::Read => return true,
            Reach::Profile { items } | Reach::Repositories { items } => items,
            Reach::Unknown => return false,
        };
        items.iter().any(|item| {
            // Backbone compares repository names without case.
            item.repo.eq_ignore_ascii_case(repo)
                && (item.access == Access::Write || access == Access::Read)
        })
    };
    if !asked.iter().all(|(repo, access)| reaches(repo, *access)) {
        return None;
    }
    let name = &agent.name;
    Some(match (&agent.reach, agent.role) {
        (Reach::Profile { .. }, Some(Role::Orchestrator))
            if asked.iter().all(|(_, access)| *access == Access::Read) =>
        {
            format!(
                "{name} is an orchestrator now, and orchestrators read every repository. Approving changes nothing."
            )
        }
        _ if asked.len() == 1 => {
            format!("{name} already reaches this repository. Approving changes nothing.")
        }
        _ => format!("{name} already reaches these repositories. Approving changes nothing."),
    })
}

/// One line of an agent's GITHUB section.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GithubRow {
    pub repo: String,
    pub access: Access,
    pub source: Source,
    /// The repository grant on it, which its remove mark takes away.
    pub grant: Option<Grant>,
}

/// The agent's GITHUB lines: what it reaches, each with the repository grant
/// on it when it holds one, then the grants its reach leaves out, such as a
/// top-level orchestrator's reads. Backbone folds grants into reach and
/// compares names without case, so every grant appears once and can be
/// taken away.
pub(super) fn github_rows(agent: &Agent) -> Vec<GithubRow> {
    let items = match &agent.reach {
        Reach::Profile { items } | Reach::Repositories { items } => &items[..],
        Reach::Unknown => &[],
    };
    let grants: Vec<(&String, Access)> = agent
        .grants
        .iter()
        .filter_map(|grant| match grant {
            Grant::Repository { repo, access } => Some((repo, *access)),
            _ => None,
        })
        .collect();
    let grant = |repo: &String, access: Access| Grant::Repository {
        repo: repo.clone(),
        access,
    };
    let mut rows: Vec<GithubRow> = items
        .iter()
        .map(|item| GithubRow {
            repo: item.repo.clone(),
            access: item.access,
            source: item.source,
            grant: grants
                .iter()
                .find(|(repo, _)| repo.eq_ignore_ascii_case(&item.repo))
                .map(|(repo, access)| grant(repo, *access)),
        })
        .collect();
    for (repo, access) in &grants {
        if !items
            .iter()
            .any(|item| item.repo.eq_ignore_ascii_case(repo))
        {
            rows.push(GithubRow {
                repo: (*repo).clone(),
                access: *access,
                source: Source::Grant,
                grant: Some(grant(repo, *access)),
            });
        }
    }
    rows
}

/// A card's key: its oldest request's ID. Requests come newest first, so a
/// newer request joining the card never changes it, and the card keeps its
/// open note and the keys' place.
pub(super) fn card_key(ids: &[i64]) -> i64 {
    ids.last().copied().unwrap_or_default()
}

/// The waiting cards the requests view draws for the requests `agent`
/// asked, or everyone's, in its order: those on the owner, then those on an
/// agent; each by its requests' IDs.
pub(super) fn card_order(pending: &[Request], agent: Option<&str>) -> Vec<Vec<i64>> {
    let listed: Vec<Request> = pending
        .iter()
        .filter(|r| agent.is_none_or(|agent| r.agent == agent))
        .cloned()
        .collect();
    let (on_you, on_agent) = waiting(&listed);
    cards(&on_you)
        .into_iter()
        .chain(cards(&on_agent))
        .map(|card| card.iter().map(|r| r.id).collect())
        .collect()
}

impl Model {
    /// The agent named `name`.
    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.overview.agents.iter().find(|agent| agent.name == name)
    }

    /// Apply a change's reply: the agent's record as Backbone holds it now.
    /// False, changing nothing, when no listed agent has its name.
    pub fn apply_access(&mut self, reply: AgentAccess) -> bool {
        let Some(agent) = self
            .overview
            .agents
            .iter_mut()
            .find(|a| a.name == reply.name)
        else {
            return false;
        };
        agent.mode = reply.mode;
        agent.grants = reply.grants;
        agent.ceiling = reply.ceiling;
        agent.reach = reply.reach;
        true
    }

    /// Show a browser as Backbone returned it, in its place or after the
    /// others.
    pub fn apply_browser(&mut self, browser: Browser) {
        // A browser made for agents that asked for it is no longer wanted.
        self.browsers
            .wanted
            .retain(|wanted| wanted.browser != browser.id);
        let items = &mut self.browsers.items;
        match items.iter_mut().find(|b| b.id == browser.id) {
            Some(listed) => *listed = browser,
            None => items.push(browser),
        }
    }

    /// Leave out a browser that was deleted.
    pub fn remove_browser(&mut self, id: &str) {
        self.browsers.items.retain(|b| b.id != id);
    }

    /// Apply a placement's reply. False when no listed agent has its name.
    pub fn apply_placed(&mut self, reply: Placed) -> bool {
        let Some(agent) = self
            .overview
            .agents
            .iter_mut()
            .find(|a| a.name == reply.name)
        else {
            return false;
        };
        agent.role = Some(reply.role);
        agent.superior = reply.superior;
        true
    }

    /// Apply a decision's reply: the request leaves the waiting ones and,
    /// once decided requests are read, leads them.
    pub fn apply_decided(&mut self, reply: Request) {
        self.pending.retain(|request| request.id != reply.id);
        if reply.state == RequestState::Pending {
            self.pending.insert(0, reply);
            return;
        }
        if let Some(decided) = &mut self.decided {
            decided.retain(|request| request.id != reply.id);
            decided.insert(0, reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_roster::profiles::access::Reached;

    fn overview() -> Overview {
        let agents: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../roster/src/profiles/access/fixtures/agents.json"
        ))
        .unwrap();
        let agents = agents["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["role"].is_string())
            .map(|item| serde_json::from_value(item.clone()).unwrap())
            .collect();
        let swarms = vec![Swarm {
            name: "app-squad".into(),
            initiator: "builder".into(),
            members: vec![
                "app-squad-coordinator".into(),
                "app-squad-coder".into(),
                "app-squad-reviewer".into(),
            ],
        }];
        Overview { agents, swarms }
    }

    fn requests(name: &str) -> Vec<Request> {
        let text = match name {
            "pending" => {
                include_str!(
                    "../../../../../roster/src/profiles/access/fixtures/requests-pending.json"
                )
            }
            _ => {
                include_str!("../../../../../roster/src/profiles/access/fixtures/requests-all.json")
            }
        };
        let page: serde_json::Value = serde_json::from_str(text).unwrap();
        serde_json::from_value(page["items"].clone()).unwrap()
    }

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-26T10:31:00+00:00").unwrap()
    }

    fn names(agents: &[&Agent]) -> Vec<String> {
        agents.iter().map(|agent| agent.name.clone()).collect()
    }

    #[test]
    fn agents_group_by_role_and_swarm_in_the_served_order() {
        let overview = overview();
        let groups = groups(&overview, Filter::All);
        assert_eq!(names(&groups.orchestrators), ["lead", "overseer"]);
        assert_eq!(names(&groups.coordinators), ["planner", "outreach"]);
        assert_eq!(names(&groups.main), ["builder", "docs-site", "tester"]);
        assert_eq!(groups.swarms.len(), 1);
        let swarm = &groups.swarms[0];
        assert_eq!(swarm.name, "app-squad");
        assert_eq!(swarm.initiator.as_deref(), Some("builder"));
        assert_eq!(
            names(&swarm.members),
            [
                "app-squad-coordinator",
                "app-squad-coder",
                "app-squad-reviewer"
            ]
        );
        assert_eq!(groups.swarm_agents(), 3);
    }

    #[test]
    fn a_swarm_agent_is_found_by_the_swarms_list_or_kept_with_main_agents() {
        let mut overview = overview();
        // Without its tag, the swarms list still places it.
        overview.agents[8].tags.clear();
        // A swarm agent no swarm names, and a role this version does not know.
        overview.agents[9].tags = vec!["swarm:".into()];
        overview.swarms[0].members.pop();
        overview.agents[6].role = Some(Role::Unknown);
        let groups = groups(&overview, Filter::All);
        assert_eq!(
            names(&groups.swarms[0].members),
            ["app-squad-coordinator", "app-squad-coder"]
        );
        assert_eq!(
            names(&groups.main),
            ["builder", "docs-site", "tester", "app-squad-reviewer"]
        );
        // A swarm the swarms read does not list has no initiator.
        overview.swarms.clear();
        let groups = super::groups(&overview, Filter::All);
        assert_eq!(groups.swarms[0].initiator, None);
    }

    #[test]
    fn filters_count_and_keep_their_agents() {
        let overview = overview();
        let agents = &overview.agents;
        let counts: Vec<_> = Filter::ALL.iter().map(|f| f.count(agents)).collect();
        assert_eq!(counts, [10, 4, 3, 1]);
        let unsandboxed = groups(&overview, Filter::Unsandboxed);
        assert_eq!(names(&unsandboxed.main), ["docs-site"]);
        assert!(unsandboxed.orchestrators.is_empty() && unsandboxed.swarms.is_empty());
        let browser = groups(&overview, Filter::Browser);
        assert_eq!(names(&browser.orchestrators), ["lead"]);
        assert_eq!(names(&browser.coordinators), ["outreach"]);
        let named = matching(&overview, Filter::All, " SQUAD-C");
        assert_eq!(
            names(&named.swarms[0].members),
            ["app-squad-coordinator", "app-squad-coder"]
        );
        assert!(named.main.is_empty() && named.orchestrators.is_empty());
    }

    #[test]
    fn rows_say_who_the_agent_answers_to_and_what_it_reaches() {
        let overview = overview();
        let agent = |name: &str| overview.agents.iter().find(|a| a.name == name).unwrap();
        assert_eq!(answers_to(agent("lead")), "answers to you");
        assert_eq!(answers_to(agent("builder")), "under planner");
        assert_eq!(reach_label(&agent("lead").reach), "every repository");
        assert_eq!(reach_label(&agent("planner").reach), "3 repositories");
        assert_eq!(reach_label(&agent("docs-site").reach), "1 repository");
        assert_eq!(reach_label(&agent("tester").reach), "no repository");
        assert_eq!(reach_label(&Reach::Unknown), "repositories not known");
        assert_eq!(
            (browsers(agent("docs-site")), secrets(agent("docs-site"))),
            (1, 2)
        );
        assert_eq!(
            role_label(agent("app-squad-coder").role.unwrap()),
            "Swarm member"
        );
    }

    #[test]
    fn the_chain_above_an_agent_ends_at_the_owner_or_a_cycle() {
        let mut overview = overview();
        assert_eq!(
            chain(&overview.agents, "app-squad-coder"),
            ["app-squad-coordinator", "builder", "planner", "lead"]
        );
        assert!(chain(&overview.agents, "lead").is_empty());
        assert!(chain(&overview.agents, "nobody").is_empty());
        // A superior that is not listed is named, and the chain ends there.
        overview.agents[6].superior = Some("gone".into());
        assert_eq!(chain(&overview.agents, "tester"), ["gone"]);
        // lead under planner under lead: each is named once.
        overview.agents[0].superior = Some("planner".into());
        assert_eq!(chain(&overview.agents, "builder"), ["planner", "lead"]);
        assert_eq!(chain(&overview.agents, "lead"), ["planner"]);
        overview.agents[6].superior = Some("tester".into());
        assert!(chain(&overview.agents, "tester").is_empty());
    }

    #[test]
    fn waiting_requests_split_by_who_decides_and_count_per_agent() {
        let pending = requests("pending");
        let (on_you, on_agent) = waiting(&pending);
        let ids: Vec<_> = on_you.iter().map(|r| r.id).collect();
        assert_eq!(ids, [21, 19]);
        assert_eq!(on_agent[0].waits_on.as_deref(), Some("builder"));
        assert_eq!(from_agents(&on_you), "2 requests from 2 agents");
        assert_eq!(from_agents(&on_agent), "1 request from 1 agent");
        assert_eq!(asks(&pending, "tester"), 1);
        assert_eq!(asks(&pending, "builder"), 0);
        // Decided requests never count as waiting.
        let all = requests("all");
        let (on_you, on_agent) = waiting(&all);
        assert_eq!(on_you.len() + on_agent.len(), 3);
    }

    #[test]
    fn waits_and_decisions_read_in_words() {
        let pending = requests("pending");
        let now = now();
        assert_eq!(
            oldest_wait(&pending, &now).as_deref(),
            Some("The oldest has waited 8 days.")
        );
        assert_eq!(oldest_wait(&[], &now), None);
        assert_eq!(asked(&pending[0], &now, false), "asked 12 minutes ago");
        assert_eq!(asked(&pending[2], &now, false), "asked 18 Nov, 8 days ago");
        assert_eq!(asked(&pending[1], &now, true), "asked 25 Nov");
        let mut hours = pending[0].clone();
        hours.requested_at = "2026-11-26T07:30:00Z".into();
        assert_eq!(asked(&hours, &now, false), "asked 3 hours ago");
        assert_eq!(
            oldest_wait(&[hours], &now).as_deref(),
            Some("The oldest has waited 3 hours.")
        );
        let all = requests("all");
        assert_eq!(decided_label(&all[3]), "approved by planner");
        assert_eq!(decided_label(&all[4]), "refused by you");
        assert_eq!(decided_label(&all[0]), "waiting");
        assert_eq!(decided_at(&all[3], &now).as_deref(), Some("21 Nov 16:02"));
        assert_eq!(decided_at(&all[0], &now), None);
        // Dates are read in the viewer's offset.
        let east = now.with_timezone(&FixedOffset::east_opt(10 * 3600).unwrap());
        assert_eq!(decided_at(&all[4], &east).as_deref(), Some("17 Nov 15:30"));
    }

    #[test]
    fn replies_replace_what_backbone_now_holds() {
        let mut model = Model {
            overview: overview(),
            pending: requests("pending"),
            ..Model::default()
        };
        let reply: AgentAccess = serde_json::from_str(include_str!(
            "../../../../../roster/src/profiles/access/fixtures/agent-access.json"
        ))
        .unwrap();
        assert!(model.apply_access(reply.clone()));
        let tester = model.agent("tester").unwrap();
        assert_eq!(tester.grants, reply.grants);
        assert_eq!(browsers(tester), 1);
        let unknown = AgentAccess {
            name: "nobody".into(),
            ..reply
        };
        let before = model.clone();
        assert!(!model.apply_access(unknown));
        assert_eq!(model, before);

        let placed = Placed {
            name: "tester".into(),
            role: Role::Coordinator,
            superior: Some("overseer".into()),
        };
        assert!(model.apply_placed(placed));
        let tester = model.agent("tester").unwrap();
        assert_eq!(tester.role, Some(Role::Coordinator));
        assert_eq!(
            chain(&model.overview.agents, "tester"),
            ["overseer", "lead"]
        );
        assert!(!model.apply_placed(Placed {
            name: "nobody".into(),
            role: Role::Main,
            superior: None,
        }));

        let mut approved = model.pending[0].clone();
        approved.state = RequestState::Approved;
        model.apply_decided(approved.clone());
        assert_eq!(model.pending.len(), 2);
        assert_eq!(model.decided, None);
        model.decided = Some(requests("all")[3..].to_vec());
        let mut refused = model.pending[0].clone();
        refused.state = RequestState::Refused;
        model.apply_decided(refused.clone());
        let decided = model.decided.as_ref().unwrap();
        assert_eq!((decided[0].id, decided.len()), (refused.id, 3));
        assert_eq!(model.pending.len(), 1);
        // A reply still waiting stays with the waiting requests.
        let mut still = model.pending[0].clone();
        still.waits_on = None;
        model.apply_decided(still);
        assert_eq!(model.pending.len(), 1);
        assert_eq!(model.pending[0].waits_on, None);

        // A reach reply replaces the listed repositories.
        let mut reply = AgentAccess {
            name: "builder".into(),
            ..AgentAccess::default()
        };
        reply.reach = Reach::Repositories {
            items: vec![Reached {
                repo: "example/app".into(),
                access: Access::Write,
                source: Source::OwnRepository,
            }],
        };
        assert!(model.apply_access(reply));
        assert_eq!(
            reach_label(&model.agent("builder").unwrap().reach),
            "1 repository"
        );
    }

    #[test]
    fn requests_of_one_kind_from_one_agent_share_a_card() {
        let mut pending = requests("pending");
        let mut second = pending[2].clone();
        second.id = 22;
        second.grant = Grant::Repository {
            repo: "example/site".into(),
            access: Access::Read,
        };
        pending.push(second);
        // Another kind from the same agent has its own card.
        let mut docker = pending[2].clone();
        docker.id = 23;
        docker.grant = Grant::Docker {
            machine: None,
            machine_name: None,
        };
        pending.push(docker);
        // So does a request for another browser.
        let mut browser = pending[0].clone();
        browser.id = 24;
        browser.grant = Grant::Browser {
            machine: Some("5f2c9e0d1b7a4e38".into()),
            machine_name: None,
            browser: Some("telegram".into()),
        };
        pending.push(browser);
        let (on_you, _) = waiting(&pending);
        let cards = cards(&on_you);
        let ids: Vec<Vec<i64>> = cards
            .iter()
            .map(|card| card.iter().map(|r| r.id).collect())
            .collect();
        assert_eq!(ids, [vec![21], vec![19, 22], vec![23], vec![24]]);
        assert_eq!(
            phrase(&cards[1]),
            ("asks to read 2 repositories".into(), None)
        );
        // A browser request names the browser.
        assert_eq!(
            phrase(&cards[0]),
            ("asks to use".into(), Some("youtube".into()))
        );
        assert_eq!(
            phrase(&[&pending[1]]),
            ("asks to read".into(), Some("example/library".into()))
        );
        assert_eq!(machine_of(&pending[0].grant).as_deref(), Some("laptop"));
        let mut secrets = pending[1].clone();
        secrets.grant = Grant::SecretVariable {
            machine: None,
            machine_name: None,
            name: "KEY".into(),
        };
        assert_eq!(
            phrase(&[&secrets]),
            ("asks for the secret".into(), Some("KEY".into()))
        );
        assert_eq!(phrase(&[&secrets, &secrets]).0, "asks for 2 secrets");
    }

    #[test]
    fn approving_what_an_agent_already_reaches_is_said_to_change_nothing() {
        let overview = overview();
        let agent = |name: &str| overview.agents.iter().find(|a| a.name == name).unwrap();
        let pending = requests("pending");
        // lead reads every repository.
        assert_eq!(
            not_needed(agent("lead"), &[&pending[2]]).as_deref(),
            Some(
                "lead is an orchestrator now, and orchestrators read every repository. Approving changes nothing."
            )
        );
        // builder already reads example/library through a grant.
        let mut library = pending[1].clone();
        library.agent = "builder".into();
        assert_eq!(
            not_needed(agent("builder"), &[&library]).as_deref(),
            Some("builder already reaches this repository. Approving changes nothing.")
        );
        // Writing it is more than it has; the browser is not a repository.
        library.grant = Grant::Repository {
            repo: "example/library".into(),
            access: Access::Write,
        };
        assert_eq!(not_needed(agent("builder"), &[&library]), None);
        assert_eq!(not_needed(agent("tester"), &[&pending[0]]), None);
        // An orchestrator's write is only needless where it already writes.
        let mut write = pending[2].clone();
        write.grant = Grant::Repository {
            repo: "example/platform".into(),
            access: Access::Write,
        };
        assert_eq!(
            not_needed(agent("lead"), &[&write]).as_deref(),
            Some("lead already reaches this repository. Approving changes nothing.")
        );
        write.grant = Grant::Repository {
            repo: "example/app".into(),
            access: Access::Write,
        };
        assert_eq!(not_needed(agent("lead"), &[&write]), None);
    }

    #[test]
    fn reading_and_writing_are_asked_on_separate_cards() {
        let pending = requests("pending");
        let mut write = pending[2].clone();
        write.id = 24;
        write.grant = Grant::Repository {
            repo: "example/b".into(),
            access: Access::Write,
        };
        let refs = [&pending[2], &write];
        let cards = cards(&refs);
        assert_eq!(cards.len(), 2);
        assert_eq!(phrase(&cards[1]).0, "asks to write");
    }

    #[test]
    fn repository_names_match_without_case() {
        let overview = overview();
        let builder = overview
            .agents
            .iter()
            .find(|a| a.name == "builder")
            .unwrap();
        let mut asked = requests("pending")[1].clone();
        asked.grant = Grant::Repository {
            repo: "Example/Library".into(),
            access: Access::Read,
        };
        assert!(not_needed(builder, &[&asked]).is_some());
    }

    #[test]
    fn the_keys_move_through_cards_in_the_drawn_order() {
        let mut pending = requests("pending");
        let mut second = pending[2].clone();
        second.id = 22;
        second.grant = Grant::Repository {
            repo: "example/site".into(),
            access: Access::Read,
        };
        pending.push(second);
        // On the owner first, then on an agent; lead's two reads share one.
        assert_eq!(
            card_order(&pending, None),
            [vec![21], vec![19, 22], vec![20]]
        );
        assert_eq!(card_order(&pending, Some("lead")), [vec![19, 22]]);
        assert!(card_order(&pending, Some("nobody")).is_empty());
    }

    #[test]
    fn requests_on_different_machines_have_their_own_cards() {
        let pending = requests("pending");
        let docker = |id: i64, machine: &str| {
            let mut request = pending[0].clone();
            request.id = id;
            request.grant = Grant::Docker {
                machine: Some(machine.into()),
                machine_name: None,
            };
            request
        };
        let (a, b, c) = (docker(30, "aa"), docker(31, "bb"), docker(32, "aa"));
        let cards = cards(&[&a, &b, &c]);
        let ids: Vec<Vec<i64>> = cards
            .iter()
            .map(|card| card.iter().map(|r| r.id).collect())
            .collect();
        assert_eq!(ids, [vec![30, 32], vec![31]]);
    }

    #[test]
    fn every_repository_grant_has_a_github_line_to_take_it_away() {
        let mut overview = overview();
        let row = |rows: &[GithubRow], repo: &str| {
            rows.iter().find(|row| row.repo == repo).cloned().unwrap()
        };
        // lead reads every repository, so its read grant is not in its reach.
        let lead = &mut overview.agents[0];
        lead.grants.push(Grant::Repository {
            repo: "example/x".into(),
            access: Access::Read,
        });
        let rows = github_rows(lead);
        assert_eq!(rows.len(), 2);
        let read = row(&rows, "example/x");
        assert_eq!((read.source, read.access), (Source::Grant, Access::Read));
        assert!(read.grant.is_some());
        // A grant on its own repository, named in another case, shows on
        // that line, which keeps its source.
        let builder = &mut overview.agents[4];
        builder.grants.push(Grant::Repository {
            repo: "Example/App".into(),
            access: Access::Read,
        });
        let rows = github_rows(builder);
        assert_eq!(rows.len(), 2);
        let own = row(&rows, "example/app");
        assert_eq!(own.source, Source::OwnRepository);
        assert_eq!(
            own.grant,
            Some(Grant::Repository {
                repo: "Example/App".into(),
                access: Access::Read,
            })
        );
        // A coordinator's read grant on a team repository stays a role line.
        let planner = &mut overview.agents[2];
        planner.grants.push(Grant::Repository {
            repo: "example/site".into(),
            access: Access::Read,
        });
        let rows = github_rows(planner);
        assert_eq!(rows.len(), 3);
        let team = row(&rows, "example/site");
        assert_eq!(team.source, Source::Role);
        assert!(team.grant.is_some());
        // A line its role gives without a grant has nothing to take away.
        assert!(row(&rows, "example/app").grant.is_none());
    }

    #[test]
    fn a_newer_request_joining_a_card_keeps_its_key() {
        let mut pending = requests("pending");
        let before = card_order(&pending, Some("lead"));
        let mut newer = pending[2].clone();
        newer.id = 40;
        newer.grant = Grant::Repository {
            repo: "example/site".into(),
            access: Access::Read,
        };
        // The newest request comes first.
        pending.insert(0, newer);
        let after = card_order(&pending, Some("lead"));
        assert_eq!(after, [vec![40, 19]]);
        assert_eq!(card_key(&after[0]), card_key(&before[0]));
    }

    #[test]
    fn a_machine_chip_never_shows_a_machine_id() {
        let label = |id, name| machine_label("this Mac", "aa", id, name);
        assert_eq!(label(Some("aa"), Some("laptop")), "this Mac");
        assert_eq!(label(None, None), "this Mac");
        assert_eq!(label(Some("bb"), Some("build-box")), "build-box");
        assert_eq!(label(Some("0d9e8f7a6b5c4d3e"), None), "another machine");
    }
}
