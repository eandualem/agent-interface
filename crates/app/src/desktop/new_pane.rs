//! The new-pane chooser: what an empty pane offers before anything runs.
//!
//! Buttons open a terminal and, in a workspace with a profile, the Reports &
//! updates sections or the other Backbone views. Under a filter, the sessions list shows Backbone agents
//! first, needing you first, then the other tmux sessions; the other lists show
//! what a pane can display instead of a terminal. Nothing is created until a choice is made, and the owner makes
//! it from the [`NewPaneEvent`] this emits.

use std::{collections::HashSet, path::PathBuf};

use agent_interface_core::{View, ViewKind};
use agent_interface_roster::{
    Swarm,
    profiles::{Connection, Error as ProfileError},
    views::{Data, Member, Team},
};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, ScrollHandle, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
};

use super::{
    agent_groups::{self, MachineHeader},
    agent_status::AgentState,
    chooser_source::{Origin, Sessions},
    peer_inbox::Selection,
    views::{peer_rows, source::Source},
};
use super::{
    components::{self as ui, mono},
    design::{colors, tokens},
    name::{NameEvent, NameField},
    views::Section,
};

/// What the person chose.
pub(super) enum NewPaneEvent {
    OpenTerminal,
    Attach(String),
    Remote(String, String),
    OpenView(View),
    /// Reports & updates, opening on one section.
    OpenSection(Section),
    /// The Browser view of this agent's browser.
    Browser(String),
    /// This swarm's members together, as a tab.
    Swarm {
        initiator: String,
        name: String,
        members: Vec<String>,
    },
    /// Connections here, on this connection or request, or on none.
    Connections(Option<Selection>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Sessions,
    /// The sections of Reports & updates.
    Reports,
    Views,
    /// Choosing the agent for an agent view.
    Agents,
}

/// One entry of the views list, in the design's order.
struct ViewChoice {
    glyph: &'static str,
    label: &'static str,
    description: &'static str,
    scope: &'static str,
    kind: ViewKind,
}

const VIEWS: [ViewChoice; 9] = [
    ViewChoice {
        glyph: "◆",
        label: "Skills",
        description: "Every skill by layer: store, repositories, user-level, and its tags",
        scope: "whole team",
        kind: ViewKind::Skills,
    },
    ViewChoice {
        glyph: "≡",
        label: "Policies",
        description: "Base, global, tag and swarm layers, each readable in full",
        scope: "whole team",
        kind: ViewKind::Policies,
    },
    ViewChoice {
        glyph: "#",
        label: "Tags",
        description: "What each tag gives the agents that carry it",
        scope: "whole team",
        kind: ViewKind::Tags,
    },
    ViewChoice {
        glyph: "⌥",
        label: "Team",
        description: "Agents by repository, tag or swarm",
        scope: "whole team",
        kind: ViewKind::Team,
    },
    ViewChoice {
        glyph: "Σ",
        label: "Usage",
        description: "Tokens by agent, CLI session and model",
        scope: "whole team",
        kind: ViewKind::Usage,
    },
    ViewChoice {
        glyph: "∿",
        label: "Activity",
        description: "State timeline, deliveries, held and expired messages",
        scope: "whole team",
        kind: ViewKind::Activity,
    },
    ViewChoice {
        glyph: "⚿",
        label: "Agent",
        description: "One agent: overview and connections",
        scope: "pick an agent",
        kind: ViewKind::Agent,
    },
    ViewChoice {
        glyph: "⇄",
        label: "Connections",
        description: "Requests and reviews with other profiles",
        scope: "this profile",
        kind: ViewKind::Connections,
    },
    ViewChoice {
        glyph: "⚷",
        label: "Access",
        description: "What each agent may reach, and requests for more",
        scope: "this profile",
        kind: ViewKind::Access,
    },
];

/// One Reports & updates section, in the hub bar's order.
struct SectionChoice {
    glyph: &'static str,
    label: &'static str,
    description: &'static str,
    scope: &'static str,
    section: Section,
}

const SECTIONS: [SectionChoice; 7] = [
    SectionChoice {
        glyph: "!",
        label: "Waiting on you",
        description: "Owner blockers from each agent's latest report",
        scope: "needs you first",
        section: Section::Waiting,
    },
    SectionChoice {
        glyph: "⇢",
        label: "Current plans",
        description: "What each agent is doing now: done, now, next, and what it waits on",
        scope: "one per agent",
        section: Section::Plans,
    },
    SectionChoice {
        glyph: "⇄",
        label: "Interactions",
        description: "Recorded messages between agents and you, with delivery history",
        scope: "one agent at a time",
        section: Section::Interactions,
    },
    SectionChoice {
        glyph: "◷",
        label: "Recent updates",
        description: "Progress reports: goal, progress, blocker and next",
        scope: "newest first",
        section: Section::Updates,
    },
    SectionChoice {
        glyph: "↳",
        label: "Responses",
        description: "Answers to questions you asked an agent",
        scope: "newest first",
        section: Section::Responses,
    },
    SectionChoice {
        glyph: "¶",
        label: "Documents",
        description: "Authored documents with their revisions and a conversation beside each",
        scope: "by author",
        section: Section::Documents,
    },
    SectionChoice {
        glyph: "▦",
        label: "Sheets",
        description: "Workbooks agents keep: read-only, with a conversation about the cells",
        scope: "by author",
        section: Section::Sheets,
    },
];

enum Row {
    Heading(&'static str, &'static str),
    /// A line of the agents tree; whether it can be chosen, as a review
    /// whose output this pane cannot show cannot; and the ID of the machine
    /// of the agent it names, so a same-name agent listed again on another
    /// machine never takes its place.
    Tree(Box<agent_groups::Row>, bool, String),
    /// A connection or request, or with none, all of them.
    Connection(Option<Selection>),
    Session(String),
    View(usize),
    Section(usize),
    Pick(View),
}

/// The agents tree as the list shows it. Under a filter, every row that
/// `matches` and the rows above it, whatever is folded; the folds are kept
/// for when the filter is cleared.
fn agent_rows(
    entries: &[(String, AgentState)],
    swarms: &[Swarm],
    started: impl Fn(&str) -> agent_groups::Started,
    machine: impl Fn(&str) -> Option<MachineHeader>,
    (expanded, collapsed): (&HashSet<String>, &HashSet<String>),
    matches: Option<&dyn Fn(&agent_groups::Row) -> bool>,
) -> Vec<agent_groups::Row> {
    let Some(matches) = matches else {
        return agent_groups::tree(entries, swarms, started, machine, expanded, collapsed);
    };
    let every: HashSet<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    let rows = agent_groups::tree(entries, swarms, started, machine, &every, &HashSet::new());
    let mut keep = vec![false; rows.len()];
    // The rows above the current one, one per level.
    let mut path: Vec<usize> = Vec::new();
    for (at, row) in rows.iter().enumerate() {
        path.truncate(row.depth());
        path.push(at);
        if matches(row) {
            for above in &path {
                keep[*above] = true;
            }
        }
    }
    let mut kept: Vec<agent_groups::Row> = rows
        .into_iter()
        .zip(keep)
        .filter_map(|(row, keep)| keep.then_some(row))
        .collect();
    // Open is what shows something under it.
    for at in 0..kept.len() {
        let deeper = kept
            .get(at + 1)
            .is_some_and(|next| next.depth() > kept[at].depth());
        if let agent_groups::Row::Agent { expanded, .. }
        | agent_groups::Row::Swarm { expanded, .. } = &mut kept[at]
        {
            *expanded = deeper;
        }
    }
    kept
}

/// Whether a tree row `matches` the filter: an agent by its name, folder,
/// tags or machine tag; what it started by what its row shows.
fn tree_matches<'a>(
    row: &agent_groups::Row,
    member: impl Fn(&str) -> Option<&'a Member>,
    matches: impl Fn(&str) -> bool,
) -> bool {
    match row {
        agent_groups::Row::Agent { name, machine, .. } => {
            matches(name)
                || machine.as_ref().is_some_and(|m| matches(&m.name))
                || member(name).is_some_and(|member| {
                    matches(&member.repo) || member.tags.iter().any(|tag| matches(tag))
                })
        }
        // A member's whole name holds its short one; its machine, its tag.
        agent_groups::Row::Member { name, machine, .. } => {
            matches(name) || machine.as_ref().is_some_and(|m| matches(&m.name))
        }
        agent_groups::Row::Swarm { name, .. } => matches(name),
        agent_groups::Row::Review { review, .. } => matches(&review.reviewer),
        agent_groups::Row::Browser { label, .. } => matches(label),
    }
}

/// Whether a tree row can be chosen: a review only when its output can be
/// watched here, from this profile's server, so its agent is `local`.
fn choosable(row: &agent_groups::Row, local: impl Fn(&str) -> bool) -> bool {
    match row {
        agent_groups::Row::Review { agent, review, .. } => review.view.is_some() && local(agent),
        _ => true,
    }
}

/// The connection rows that `matches` the filter, then "All connections".
fn connection_rows(entries: &[peer_rows::Entry], matches: impl Fn(&str) -> bool) -> Vec<Row> {
    let mut rows: Vec<Row> = entries
        .iter()
        .filter(|entry| matches(peer_rows::display_name(entry.name.as_deref())))
        .map(|entry| Row::Connection(Some(entry.selection.clone())))
        .collect();
    if matches(ALL_CONNECTIONS) {
        rows.push(Row::Connection(None));
    }
    rows
}

const ALL_CONNECTIONS: &str = "All connections";

/// What a row's folder text shows: its repository, else its tags. Its
/// machine is a tag of its own.
fn folder(member: &Member) -> String {
    if member.repo.is_empty() {
        member.tags.join(" · ")
    } else {
        member.repo.clone()
    }
}

/// What → or ← does on the selected row.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Open(String),
    Close(String),
    OpenSwarm(String),
    FoldSwarm(String),
    /// Select the row with this key.
    Select(String),
    Nothing,
}

/// → opens the selected agent or swarm; ← closes it, or from a row under
/// another selects that one.
fn step(rows: &[Row], selected: &str, open: bool) -> Step {
    let Some(at) = rows.iter().position(|row| row_key(row) == selected) else {
        return Step::Nothing;
    };
    let Row::Tree(row, ..) = &rows[at] else {
        return Step::Nothing;
    };
    match (row.as_ref(), open) {
        (
            agent_groups::Row::Agent {
                name,
                children: true,
                expanded,
                ..
            },
            _,
        ) if *expanded != open => {
            if open {
                Step::Open(name.clone())
            } else {
                Step::Close(name.clone())
            }
        }
        (
            agent_groups::Row::Swarm {
                initiator,
                name,
                listed,
                expanded,
                ..
            },
            _,
        ) if *listed > 0 && *expanded != open => {
            let key = agent_groups::swarm_key(initiator, name);
            if open {
                Step::OpenSwarm(key)
            } else {
                Step::FoldSwarm(key)
            }
        }
        (row, false) if row.depth() > 0 => rows[..at]
            .iter()
            .rev()
            .find(|above| matches!(above, Row::Tree(above, ..) if above.depth() < row.depth()))
            .map_or(Step::Nothing, |above| Step::Select(row_key(above))),
        _ => Step::Nothing,
    }
}

/// Apply `step` to the list's folds and selection; whether anything changed.
fn apply(
    step: Step,
    expanded: &mut HashSet<String>,
    collapsed: &mut HashSet<String>,
    selected: &mut Option<String>,
) -> bool {
    match step {
        Step::Open(agent) => expanded.insert(agent),
        Step::Close(agent) => expanded.remove(&agent),
        Step::OpenSwarm(key) => collapsed.remove(&key),
        Step::FoldSwarm(key) => collapsed.insert(key),
        Step::Select(key) => {
            *selected = Some(key);
            true
        }
        Step::Nothing => false,
    }
}

/// Whether `key` folds the list, and which way: → and ← do in the agents
/// list while the filter is empty; once it holds text, they move its caret.
fn folding(mode: Mode, query: &str, key: &str) -> Option<bool> {
    match key {
        "right" | "left" if mode == Mode::Sessions && query.is_empty() => Some(key == "right"),
        _ => None,
    }
}

fn numbered_row(presented: &[String], number: usize, choices: Vec<Row>) -> Option<Row> {
    let key = presented.get(number.checked_sub(1)?)?;
    choices.into_iter().find(|row| row_key(row) == *key)
}

fn reconcile_selection(
    rows: &[Row],
    selected: &mut Option<String>,
    disarmed: &mut bool,
    reading: bool,
    own: impl Fn(&Row) -> bool,
) {
    if reading && selected.is_some() {
        return;
    }
    let mut choices = rows.iter().filter(|row| NewPane::selectable(row));
    if let Some(key) = selected {
        if !choices.any(|row| row_key(row) == *key) {
            *selected = None;
            *disarmed = true;
        }
    } else if !*disarmed && !reading {
        // Until every read has landed, rows change kind and place: a session
        // becomes an agent, a member listed alone folds under its agent.
        // The first choice waits; a person's own choice never does. Under a
        // filter it takes the first row that matches by itself, not one
        // kept above a match to show its place.
        let mut first = choices.clone().filter(|row| own(row));
        *selected = first.next().or_else(|| choices.next()).map(row_key);
    }
}

/// Whether the list is still being read: until its sessions, its team and,
/// for a profile, its roster with the swarms have each landed once. A read
/// that failed has landed.
fn reading(mode: Mode, sessions: bool, team: bool, roster: Option<bool>) -> bool {
    match mode {
        Mode::Sessions => sessions || team || roster == Some(false),
        Mode::Agents => team,
        Mode::Reports | Mode::Views => false,
    }
}

pub(super) fn sign_in_refusal(error: ProfileError) -> &'static str {
    match error {
        ProfileError::Refused(422) => "This CLI is not available in this profile.",
        ProfileError::Refused(503) => "The profile could not start the sign-in session.",
        ProfileError::Refused(_) => "The profile refused the sign-in.",
        _ => "The workspace profile connection is unavailable.",
    }
}

pub(super) struct NewPane {
    pub(super) placement: Option<(super::navigation_origin::Origin, String)>,
    agents: super::agent_status::LiveAgents,
    origin: Origin,
    pane: String,
    field: Entity<NameField>,
    query: String,
    mode: Mode,
    /// The chosen row by identity, so refreshes that reorder or remove rows
    /// never move Enter onto another session.
    selected: Option<String>,
    /// Number keys resolve the last rendered list, never newly arrived numbering.
    presented: Vec<String>,
    /// The chosen row disappeared; nothing is chosen until the next move.
    disarmed: bool,
    /// Scroll the chosen row into view at the next frame.
    reveal: bool,
    sessions: Vec<String>,
    sessions_error: Option<String>,
    loading: bool,
    team: Option<Team>,
    team_loading: bool,
    /// Running agent-browser sessions by name, each with its page's host.
    browsers: Vec<(String, String)>,
    /// Agents opened in this list, by name: its own folds, apart from the panel's.
    expanded: HashSet<String>,
    /// Swarms folded in this list, by [`agent_groups::swarm_key`]; a swarm is
    /// open once its agent opens.
    collapsed_swarms: HashSet<String>,
    /// The connections of this pane's profile, as the panel lists them.
    connections: Vec<peer_rows::Entry>,
    visible: bool,
    scroll: ScrollHandle,
    session_source: Option<Sessions>,
    team_source: Option<Source>,
    _updates: Task<()>,
    _input: Subscription,
}

impl EventEmitter<NewPaneEvent> for NewPane {}

impl NewPane {
    /// The chooser in a workspace without a profile, which has no agents.
    pub fn new(
        pane: String,
        socket: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with(
            pane,
            Origin::Standalone { socket },
            Default::default(),
            window,
            cx,
        )
    }

    /// The workspace owner must retain this connection when accepting events.
    pub fn new_bound(
        pane: String,
        connection: Connection,
        agents: super::agent_status::LiveAgents,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Self> {
        if !agents.belongs_to(connection.identity()) {
            return None;
        }
        Some(Self::new_with(
            pane,
            Origin::Bound(connection),
            agents,
            window,
            cx,
        ))
    }

    pub fn connection(&self) -> Option<&Connection> {
        self.origin.connection()
    }

    fn new_with(
        pane: String,
        origin: Origin,
        agents: super::agent_status::LiveAgents,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| NameField::bare_filter(window, cx));
        let input = cx.subscribe_in(&field, window, |chooser, _, event, _, cx| {
            match event {
                NameEvent::Changed(query) => {
                    chooser.query = query.clone();
                    chooser.selected = None;
                    chooser.disarmed = false;
                    chooser.scroll.scroll_to_item(0);
                }
                NameEvent::Save => chooser.activate(cx),
                NameEvent::Cancel => {}
            }
            cx.notify();
        });
        let (wake, mut woken) = mpsc::unbounded::<()>();
        let session_wake = wake.clone();
        let (session_source, sessions_error) = match origin.sessions(move || {
            let _ = session_wake.unbounded_send(());
        }) {
            Ok(source) => (source, None),
            Err(_) => (None, Some("Session discovery is unavailable.".into())),
        };
        let team_source = origin
            .team(move || {
                let _ = wake.unbounded_send(());
            })
            .ok()
            .flatten();
        let updates = cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this.update(cx, |chooser, cx| chooser.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            placement: None,
            agents,
            pane,
            field,
            query: String::new(),
            mode: Mode::Sessions,
            selected: None,
            presented: Vec::new(),
            disarmed: false,
            reveal: false,
            sessions: Vec::new(),
            sessions_error,
            loading: session_source.is_some(),
            team: None,
            team_loading: team_source.is_some(),
            browsers: Vec::new(),
            expanded: HashSet::new(),
            collapsed_swarms: HashSet::new(),
            connections: Vec::new(),
            visible: true,
            origin,
            scroll: ScrollHandle::new(),
            session_source,
            team_source,
            _updates: updates,
            _input: input,
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if let Some(source) = &self.session_source {
            source.set_active(visible);
        }
        if let Some(source) = &self.team_source {
            source.set_active(visible);
        }
        self.sessions.clear();
        if self.session_source.is_some() {
            self.sessions_error = None;
        }
        self.team = None;
        self.loading = self.session_source.is_some();
        self.team_loading = self.team_source.is_some();
        cx.notify();
    }

    /// The running agent-browser sessions, by name with their pages' hosts.
    pub fn set_browsers(&mut self, browsers: Vec<(String, String)>, cx: &mut Context<Self>) {
        if self.browsers != browsers {
            self.browsers = browsers;
            cx.notify();
        }
    }

    /// The connections of this pane's profile.
    pub fn set_connections(&mut self, connections: Vec<peer_rows::Entry>, cx: &mut Context<Self>) {
        if self.connections != connections {
            self.connections = connections;
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if let Some(observation) = self.session_source.as_ref().and_then(Sessions::take_update) {
            self.sessions = observation.sessions;
            self.sessions_error = observation.error;
            self.loading = false;
            cx.notify();
        }
        if let Some((_, update)) = self.team_source.as_ref().and_then(Source::take_update) {
            self.team_loading = false;
            // Without a current read, show plain sessions rather than old states.
            self.team = match update {
                Ok(Data::Team(team)) => Some(team),
                _ => None,
            };
            cx.notify();
        }
    }

    fn matches(&self, text: &str) -> bool {
        self.query.is_empty() || text.to_lowercase().contains(&self.query.to_lowercase())
    }

    /// Local sessions and remote agent streams, needing attention first.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        match self.mode {
            Mode::Sessions => {
                let mut entries: Vec<(String, AgentState)> = Vec::new();
                for member in self
                    .team
                    .iter()
                    .flat_map(|team| team.members.iter())
                    .filter(|member| self.listed(member))
                {
                    if entries.iter().all(|(name, _)| *name != member.name) {
                        entries.push((member.name.clone(), self.agents.get(&member.name).state));
                    }
                }
                let local = |name: &str| self.member(name).is_some() && self.remote(name).is_none();
                let local_machine = self.agents.local_machine_id();
                let (machines, machines_read) = self.agents.machines();
                let matches = |row: &agent_groups::Row| {
                    tree_matches(row, |name| self.member(name), |text| self.matches(text))
                };
                let tree = agent_rows(
                    &entries,
                    &self.agents.swarms(),
                    |name| agent_groups::Started {
                        reviews: self
                            .agents
                            .activity(name)
                            .map(|activity| activity.reviews.clone())
                            .unwrap_or_default(),
                        browser: self
                            .browsers
                            .iter()
                            .find(|(agent, _)| agent == name && local(name))
                            .map(|(_, host)| host.clone()),
                    },
                    |name| {
                        agent_groups::machine_tag(
                            local_machine.as_deref(),
                            &machines,
                            machines_read,
                            self.member(name)?.machine.as_ref(),
                        )
                    },
                    (&self.expanded, &self.collapsed_swarms),
                    (!self.query.is_empty())
                        .then_some(&matches as &dyn Fn(&agent_groups::Row) -> bool),
                );
                let others: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|name| {
                        !self
                            .team
                            .iter()
                            .any(|team| team.members.iter().any(|m| &&m.name == name))
                    })
                    .filter(|name| self.matches(name))
                    .cloned()
                    .collect();
                if !tree.is_empty() {
                    rows.push(Row::Heading("BACKBONE AGENTS", "needs you first"));
                    rows.extend(tree.into_iter().map(|row| {
                        let choosable = choosable(&row, local);
                        let machine = match &row {
                            agent_groups::Row::Agent { name, .. }
                            | agent_groups::Row::Member { name, .. } => self
                                .member(name)
                                .and_then(|member| member.machine.as_ref())
                                .map(|machine| machine.id.clone())
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        Row::Tree(Box::new(row), choosable, machine)
                    }));
                }
                if self.origin.has_provider() {
                    let connections = connection_rows(&self.connections, |text| self.matches(text));
                    if !connections.is_empty() {
                        rows.push(Row::Heading("CONNECTIONS", "other profiles"));
                        rows.extend(connections);
                    }
                }
                if !others.is_empty() {
                    rows.push(Row::Heading(
                        if self.team.is_some() {
                            "OTHER TMUX SESSIONS"
                        } else {
                            "TMUX SESSIONS"
                        },
                        "",
                    ));
                    rows.extend(others.into_iter().map(Row::Session));
                }
            }
            Mode::Reports => {
                rows.extend(
                    SECTIONS
                        .iter()
                        .enumerate()
                        .filter(|(_, section)| self.matches(section.label))
                        .map(|(index, _)| Row::Section(index)),
                );
            }
            Mode::Views => {
                rows.extend(
                    VIEWS
                        .iter()
                        .enumerate()
                        .filter(|(_, view)| self.origin.has_provider() && self.matches(view.label))
                        .map(|(index, _)| Row::View(index)),
                );
            }
            Mode::Agents => {
                let mut views: Vec<View> = self
                    .team
                    .iter()
                    .flat_map(|team| team.members.iter())
                    .filter(|member| self.matches(&member.name))
                    .filter_map(|member| self.origin.agent_view(member))
                    .collect();
                views.sort_by_cached_key(|view| {
                    view.agent
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                });
                rows.extend(views.into_iter().map(Row::Pick));
            }
        }
        rows
    }

    fn selectable(row: &Row) -> bool {
        !matches!(row, Row::Heading(..) | Row::Tree(_, false, _))
    }

    /// Whether number keys reach the row: only top-level rows.
    fn numbered(row: &Row) -> bool {
        Self::selectable(row) && !matches!(row, Row::Tree(row, ..) if row.depth() > 0)
    }

    /// The listed team member of this name.
    fn member(&self, name: &str) -> Option<&Member> {
        self.team
            .iter()
            .flat_map(|team| team.members.iter())
            .find(|member| member.name == name && self.listed(member))
    }

    /// The profile a listed member's remote terminal opens on, when it is
    /// on another machine of the profile.
    fn remote(&self, name: &str) -> Option<String> {
        self.origin.remote_profile(self.member(name)?)
    }

    /// Whether a team member is listed: it has a session here, or it is
    /// opened from another machine of the profile.
    fn listed(&self, member: &Member) -> bool {
        self.origin.remote_profile(member).is_some() || self.sessions.contains(&member.name)
    }

    /// → or ← on the selected row.
    fn fold(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let step = step(&self.rows(), &selected, open);
        let select = matches!(step, Step::Select(_));
        if !apply(
            step,
            &mut self.expanded,
            &mut self.collapsed_swarms,
            &mut self.selected,
        ) {
            return;
        }
        if select {
            self.disarmed = false;
            self.reveal = true;
        }
        cx.notify();
    }

    fn choices(&self) -> Vec<Row> {
        self.rows().into_iter().filter(Self::selectable).collect()
    }

    fn selected_index(&self, choices: &[Row]) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        choices.iter().position(|row| row_key(row) == selected)
    }

    /// Move the selection by `offset` choices and scroll it into view.
    pub fn move_by(&mut self, offset: isize, cx: &mut Context<Self>) {
        let choices = self.choices();
        if choices.is_empty() {
            return;
        }
        let next = match self.selected_index(&choices) {
            Some(at) => (at as isize + offset).clamp(0, choices.len() as isize - 1) as usize,
            None => 0,
        };
        self.selected = Some(row_key(&choices[next]));
        self.disarmed = false;
        self.reveal = true;
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.selected = None;
        self.disarmed = false;
        // A filter typed for one list means nothing in the next.
        self.query.clear();
        self.field.update(cx, |field, cx| field.clear(cx));
        cx.notify();
    }

    /// Move through sessions, Reports & updates and the views, forward or back;
    /// the agent list counts as the views.
    pub fn toggle(&mut self, back: bool, cx: &mut Context<Self>) {
        if !self.origin.has_provider() {
            return;
        }
        self.set_mode(cycle(self.mode, back), cx);
    }

    /// Show `mode`'s list, with keys going to it rather than a terminal beside it.
    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.field.read(cx).focus.clone();
        window.focus(&focus, cx);
        if self.mode != mode {
            self.set_mode(mode, cx);
        }
    }

    /// Choose the selected row.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
        let mut choices = self.choices();
        if let Some(at) = self.selected_index(&choices) {
            let row = choices.swap_remove(at);
            self.choose(row, cx);
        }
    }

    /// Choose the numbered row, 1 to 9.
    pub fn choose_number(&mut self, number: usize, cx: &mut Context<Self>) {
        self.refresh(cx);
        if let Some(row) = numbered_row(&self.presented, number, self.choices()) {
            self.choose(row, cx);
        }
    }

    fn choose(&mut self, row: Row, cx: &mut Context<Self>) {
        match row {
            Row::Heading(..) | Row::Tree(_, false, _) => {}
            Row::Tree(row, true, _) => match *row {
                agent_groups::Row::Agent { name, .. } | agent_groups::Row::Member { name, .. } => {
                    match self.remote(&name) {
                        Some(remote) => cx.emit(NewPaneEvent::Remote(name, remote)),
                        None => cx.emit(NewPaneEvent::Attach(name)),
                    }
                }
                agent_groups::Row::Browser { agent, .. } => cx.emit(NewPaneEvent::Browser(agent)),
                agent_groups::Row::Swarm {
                    initiator,
                    name,
                    members,
                    ..
                } => cx.emit(NewPaneEvent::Swarm {
                    initiator,
                    name,
                    members,
                }),
                // Its view on this profile's server, which opens read-only.
                agent_groups::Row::Review { review, .. } => {
                    if let Some(view) = review.view {
                        cx.emit(NewPaneEvent::Attach(view));
                    }
                }
            },
            Row::Connection(selection) => cx.emit(NewPaneEvent::Connections(selection)),
            Row::Session(name) => cx.emit(NewPaneEvent::Attach(name)),
            Row::View(index) => match VIEWS[index].kind {
                ViewKind::Agent => self.set_mode(Mode::Agents, cx),
                ViewKind::Connections => cx.emit(NewPaneEvent::Connections(None)),
                kind => cx.emit(NewPaneEvent::OpenView(View::of(kind))),
            },
            Row::Section(index) => cx.emit(NewPaneEvent::OpenSection(SECTIONS[index].section)),
            Row::Pick(view) => cx.emit(NewPaneEvent::OpenView(view)),
        }
    }

    pub fn pane(&self) -> &str {
        &self.pane
    }
}

impl Drop for NewPane {
    fn drop(&mut self) {
        // Both workers may be inside a bounded read; never wait on the UI thread.
        let sources = (self.session_source.take(), self.team_source.take());
        std::thread::spawn(move || drop(sources));
    }
}

impl Render for NewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let rows = self.rows();
        self.presented = rows
            .iter()
            .filter(|row| Self::numbered(row))
            .map(row_key)
            .collect();
        let count = rows.iter().filter(|row| Self::selectable(row)).count();
        let reading = reading(
            self.mode,
            self.loading,
            self.team_loading,
            self.origin.has_provider().then(|| self.agents.observed()),
        );
        let own: HashSet<String> = rows
            .iter()
            .filter(|row| match row {
                Row::Tree(line, ..) => {
                    self.query.is_empty()
                        || tree_matches(line, |name| self.member(name), |text| self.matches(text))
                }
                _ => true,
            })
            .map(row_key)
            .collect();
        reconcile_selection(
            &rows,
            &mut self.selected,
            &mut self.disarmed,
            reading,
            |row| own.contains(&row_key(row)),
        );
        let views = matches!(self.mode, Mode::Views | Mode::Agents);
        let buttons = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .gap(px(t.space.s10))
            .child(
                ui::choice_button(
                    "new-pane-terminal",
                    "terminal",
                    "Open terminal",
                    if cfg!(target_os = "macos") {
                        "⌘↵"
                    } else {
                        "Ctrl+↵"
                    },
                    false,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(NewPaneEvent::OpenTerminal);
                })),
            )
            .when(self.origin.has_provider(), |buttons| {
                buttons
                    .child(
                        ui::choice_button(
                            "new-pane-reports",
                            "message-square",
                            "Reports & updates",
                            "⇥",
                            self.mode == Mode::Reports,
                        )
                        .on_click(cx.listener(|chooser, _, window, cx| {
                            cx.stop_propagation();
                            chooser.show(Mode::Reports, window, cx);
                        })),
                    )
                    .child(
                        ui::choice_button(
                            "new-pane-views",
                            "layout-grid",
                            "Skills, usage, team…",
                            "⇧⇥",
                            views,
                        )
                        .on_click(cx.listener(|chooser, _, window, cx| {
                            cx.stop_propagation();
                            let mode = match chooser.mode {
                                Mode::Agents => Mode::Agents,
                                _ => Mode::Views,
                            };
                            chooser.show(mode, window, cx);
                        })),
                    )
            });
        let mut list = div()
            .id(SharedString::from(format!("new-pane-rows-{}", self.pane)))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s2))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if self.mode == Mode::Reports {
            list = list.child(
                ui::view_heading("REPORTS & UPDATES · FROM AGENT BACKBONE", "").child(back(
                    &self.pane,
                    "← back to sessions",
                    Mode::Sessions,
                    cx,
                )),
            );
        } else if self.mode == Mode::Views {
            list = list.child(ui::view_heading("AGENT BACKBONE", "").child(back(
                &self.pane,
                "← back to sessions",
                Mode::Sessions,
                cx,
            )));
        } else if self.mode == Mode::Agents {
            list = list.child(ui::view_heading("WHICH AGENT?", "").child(back(
                &self.pane,
                "← views",
                Mode::Views,
                cx,
            )));
        }
        let mut number = 0;
        let mut selected_remote = false;
        // Children before the rows: the list's heading in the views lists.
        for (child, row) in (usize::from(self.mode != Mode::Sessions)..).zip(rows) {
            let selectable = Self::selectable(&row);
            let selected = selectable && self.selected.as_deref() == Some(row_key(&row).as_str());
            let numbered = Self::numbered(&row);
            if numbered {
                number += 1;
            }
            let key = if numbered && number <= 9 {
                number.to_string()
            } else {
                String::new()
            };
            let chosen = row_key(&row);
            if selected && std::mem::take(&mut self.reveal) {
                self.scroll.scroll_to_item(child);
            }
            let id =
                SharedString::from(format!("new-pane-{}-{number}-{}", self.pane, row_key(&row)));
            let element = match &row {
                Row::Heading(label, note) => ui::view_heading(*label, *note).into_any_element(),
                Row::Tree(line, choosable, _) => {
                    if selected
                        && let agent_groups::Row::Agent { name, .. }
                        | agent_groups::Row::Member { name, .. } = line.as_ref()
                    {
                        selected_remote = self.remote(name).is_some();
                    }
                    let depth = line.depth();
                    // A top-level row has the chevron's slot before its symbol.
                    ui::nested(
                        depth,
                        self.tree_row(id, line, (key, chosen.clone(), *choosable), selected, cx),
                        t.space.s20 + t.space.s10,
                        // Every level's line goes on through the gap above.
                        (t.space.s2, depth),
                    )
                }
                Row::Connection(selection) => self
                    .connection_row(id, selection.as_ref(), key, selected)
                    .into_any_element(),
                Row::Session(name) => ui::choice_frame(id, selected)
                    .child(ui::icon("terminal", t.size.icon, c.muted))
                    .child(mono(name.clone()).flex_1().text_color(c.text))
                    .child(ui::choice_key(key))
                    .into_any_element(),
                Row::View(view) => {
                    let view = &VIEWS[*view];
                    ui::view_choice(
                        id,
                        view.glyph,
                        view.label,
                        view.description,
                        view.scope,
                        key,
                        selected,
                    )
                    .into_any_element()
                }
                Row::Section(section) => {
                    let section = &SECTIONS[*section];
                    ui::view_choice(
                        id,
                        section.glyph,
                        section.label,
                        section.description,
                        section.scope,
                        key,
                        selected,
                    )
                    .into_any_element()
                }
                Row::Pick(view) => ui::choice_frame(id, selected)
                    .child(ui::icon("user", t.size.icon, c.muted))
                    .child(
                        mono(view.agent.clone().unwrap_or_default())
                            .flex_1()
                            .text_color(c.text),
                    )
                    .child(ui::choice_key(key))
                    .into_any_element(),
            };
            list = list.child(if selectable {
                div()
                    .id(SharedString::from(format!(
                        "new-pane-pick-{}-{chosen}",
                        self.pane
                    )))
                    .on_click(cx.listener(move |chooser, _, _, cx| {
                        cx.stop_propagation();
                        chooser.selected = Some(chosen.clone());
                        chooser.activate(cx);
                    }))
                    .child(element)
                    .into_any_element()
            } else {
                element
            });
        }
        let status = if self.mode == Mode::Sessions {
            if let Some(error) = &self.sessions_error {
                Some(error.clone())
            } else if self.loading {
                Some("Looking for sessions…".to_owned())
            } else if count == 0 {
                Some("No session matches the filter.".to_owned())
            } else {
                None
            }
        } else if self.mode == Mode::Agents && count == 0 {
            Some("No agent matches. Backbone may not have answered yet.".to_owned())
        } else {
            None
        };
        if let Some(status) = status {
            list = list.child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .py(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.body,
                )
                .child(status),
            );
        }
        let (enter, tab): (SharedString, SharedString) = match self.mode {
            Mode::Sessions if selected_remote => {
                ("open remote terminal".into(), "reports & updates".into())
            }
            Mode::Sessions => ("attach here".into(), "reports & updates".into()),
            Mode::Reports => ("open here".into(), "skills, usage, team…".into()),
            Mode::Views => ("open view here".into(), "back to sessions".into()),
            Mode::Agents => ("open its card here".into(), "back to sessions".into()),
        };
        let mut hints = vec![("↑↓", SharedString::from("move"))];
        if self.origin.has_provider() && self.mode == Mode::Sessions {
            hints.push(("→", "expand".into()));
            hints.push(("←", "collapse".into()));
        }
        hints.push(("↵", enter));
        if self.origin.has_provider() {
            hints.push(("⇥", tab));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s12))
                    .pt(px(t.space.s20))
                    .px(px(t.space.s28))
                    .child(buttons)
                    .child(ui::filter_frame().child(
                        if self.mode == Mode::Sessions && self.origin.has_provider() {
                            peer_rows::hinted(
                                &self.field,
                                "Filter agents, what they started, connections and sessions",
                                cx,
                            )
                        } else {
                            div().flex_1().min_w_0().child(self.field.clone())
                        },
                    ))
                    .child(list),
            )
            .child(ui::hint_bar(&hints, "Nothing is created until you choose."))
            .capture_key_down(cx.listener(|chooser, event: &gpui::KeyDownEvent, _, cx| {
                if chooser.field.read(cx).composing() {
                    return;
                }
                if chooser.key(&event.keystroke, false, cx) {
                    cx.stop_propagation();
                }
            }))
    }
}

impl NewPane {
    /// Handle a chooser key; returns whether it was one. The window also sends
    /// these while navigation has focus and this pane is selected.
    /// Number keys choose only from navigation; in the filter they are text.
    pub fn key(&mut self, key: &gpui::Keystroke, navigating: bool, cx: &mut Context<Self>) -> bool {
        let m = &key.modifiers;
        // The command key on macOS, Control elsewhere, as the button says.
        let (command, other) = if cfg!(target_os = "macos") {
            (m.platform, m.control)
        } else {
            (m.control, m.platform)
        };
        if key.key == "enter" && command && !other && !m.alt {
            cx.emit(NewPaneEvent::OpenTerminal);
            return true;
        }
        if key.key == "tab" && m.shift && !m.control && !m.alt && !m.platform && !m.function {
            self.toggle(true, cx);
            return true;
        }
        if m.modified() {
            return false;
        }
        if let Some(open) = folding(self.mode, &self.query, &key.key) {
            self.fold(open, cx);
            return true;
        }
        match key.key.as_str() {
            "up" => self.move_by(-1, cx),
            "down" => self.move_by(1, cx),
            "tab" => self.toggle(false, cx),
            "enter" => self.activate(cx),
            digit if navigating && digit.len() == 1 => match digit.parse::<usize>() {
                Ok(number) if number > 0 => self.choose_number(number, cx),
                _ => return false,
            },
            _ => return false,
        }
        true
    }
}

fn back(
    pane: &str,
    label: &'static str,
    to: Mode,
    cx: &mut Context<NewPane>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("new-pane-back-{pane}")))
        .cursor_pointer()
        .text_color(colors().accent_text)
        .child(label)
        .on_click(cx.listener(move |chooser, _, _, cx| {
            cx.stop_propagation();
            chooser.set_mode(to, cx);
        }))
}

/// The list Tab moves to from `mode`, or Shift-Tab when `back`.
fn cycle(mode: Mode, back: bool) -> Mode {
    match (mode, back) {
        (Mode::Sessions, false) | (Mode::Views | Mode::Agents, true) => Mode::Reports,
        (Mode::Reports, false) | (Mode::Sessions, true) => Mode::Views,
        (Mode::Views | Mode::Agents, false) | (Mode::Reports, true) => Mode::Sessions,
    }
}

fn row_key(row: &Row) -> String {
    match row {
        Row::Heading(label, _) => (*label).to_owned(),
        Row::Tree(row, _, machine) => tree_key(row, machine),
        // tmux names hold no colon, so no session takes this.
        Row::Connection(None) => "connections:all".to_owned(),
        Row::Connection(Some(Selection::Peer(generation))) => {
            format!("connection:{}", generation.generation)
        }
        Row::Connection(Some(Selection::Request(request))) => format!("request:{request}"),
        Row::Session(name) => name.clone(),
        Row::Pick(view) => format!(
            "agent:{}:{}",
            view.agent.as_deref().unwrap_or_default(),
            view.registration_id.as_deref().unwrap_or_default()
        ),
        Row::View(index) => VIEWS[*index].label.to_owned(),
        Row::Section(index) => SECTIONS[*index].label.to_owned(),
    }
}

/// A tree row's identity, with the ID of the machine of the agent it names.
/// Names never hold a slash or a colon, so these cannot collide.
fn tree_key(row: &agent_groups::Row, machine: &str) -> String {
    match row {
        agent_groups::Row::Agent { name, .. } => format!("agent:{name}:{machine}"),
        agent_groups::Row::Member { swarm, name, .. } => {
            format!("member:{swarm}/{name}:{machine}")
        }
        // By its view, so a review ending above it never moves the selection.
        agent_groups::Row::Review {
            swarm,
            review: agent_groups::Review {
                view: Some(view), ..
            },
            ..
        } => format!("review:{}/{view}", swarm.as_deref().unwrap_or_default()),
        agent_groups::Row::Swarm {
            initiator, name, ..
        } => format!("swarm:{}", agent_groups::swarm_key(initiator, name)),
        agent_groups::Row::Review {
            agent,
            swarm,
            index,
            ..
        } => format!(
            "review:{}/{agent}/{index}",
            swarm.as_deref().unwrap_or_default()
        ),
        agent_groups::Row::Browser { agent, swarm, .. } => {
            format!("browser:{}/{agent}", swarm.as_deref().unwrap_or_default())
        }
    }
}

/// What Enter does on a selected row under an agent.
fn attach_hint(label: &'static str) -> Div {
    let t = tokens();
    ui::text(
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(t.space.s4))
            .text_color(colors().muted),
        &t.text.meta,
    )
    .child(ui::key_hint("↵"))
    .child(label)
}

/// Text in `meta` that gives way first.
fn meta(text: impl Into<SharedString>, color: gpui::Rgba) -> Div {
    ui::text(
        div().flex_shrink_0().whitespace_nowrap().text_color(color),
        &tokens().text.meta,
    )
    .child(text.into())
}

/// A child row's frame that cannot be chosen: a `choice_frame`'s layout,
/// without its pointer, hover or selection.
fn quiet_frame(id: SharedString) -> gpui::Stateful<Div> {
    let t = tokens();
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s10))
        .h(px(t.size.row))
        .px(px(t.space.s10))
        .rounded(px(t.radius.row))
        .relative()
}

/// The slot before a top-level row's symbol, holding its chevron.
fn chevron_slot() -> Div {
    div()
        .flex_shrink_0()
        .w(px(tokens().space.s20))
        .flex()
        .justify_center()
}

/// A fold's chevron as a button.
fn chevron(id: String, open: bool) -> gpui::Stateful<Div> {
    chevron_mark(open)
        .id(SharedString::from(id))
        .cursor_pointer()
        .hover(|el| el.bg(colors().selected))
}

/// A fold's chevron showing only its state, as under a filter.
fn chevron_mark(open: bool) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .size(px(t.space.s20))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(t.radius.icon_button))
        .child(ui::icon(
            if open {
                "chevron-down"
            } else {
                "chevron-right"
            },
            t.size.icon,
            c.muted,
        ))
}

impl NewPane {
    /// A line of the agents tree.
    fn tree_row(
        &self,
        id: SharedString,
        line: &agent_groups::Row,
        (key, chosen, choosable): (String, String, bool),
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let child = || ui::choice_frame(id.clone(), selected).h(px(t.size.row));
        let tag = |machine: &Option<MachineHeader>| {
            machine
                .as_ref()
                .map(|machine| ui::machine_tag(&machine.name, machine.presence.attention()))
        };
        let fill = || div().flex_1().min_w_0();
        // A review's, browser's or swarm's symbol and label, as the panel's.
        let (symbol, label) = ui::tree_line(line).unzip();
        // A label that takes the room left.
        let filling = |label: Div| {
            label
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
        };
        match line {
            agent_groups::Row::Agent {
                name,
                state,
                machine,
                counts,
                children,
                expanded,
            } => {
                let member = self.member(name);
                let status = self.agents.get(name);
                let toggled = name.clone();
                let open = *expanded;
                // A filter shows what matches and never folds: there the
                // chevron only shows the state.
                let filtered = !self.query.is_empty();
                let slot = chevron_slot()
                    .when(*children && filtered, |slot| slot.child(chevron_mark(open)));
                let slot = slot.when(*children && !filtered, |slot| {
                    slot.child(
                        chevron(format!("new-pane-fold-{}-{name}", self.pane), open).on_click(
                            cx.listener(move |chooser, _, _, cx| {
                                cx.stop_propagation();
                                if chooser.expanded.remove(&toggled) {
                                    // Closed, it keeps the selection, on itself.
                                    chooser.selected = Some(chosen.clone());
                                    chooser.disarmed = false;
                                } else {
                                    chooser.expanded.insert(toggled.clone());
                                }
                                cx.notify();
                            }),
                        ),
                    )
                });
                ui::choice_frame(id, selected)
                    .child(slot)
                    .child(ui::agent_symbol(ui::shown_state(*state, status.stale)))
                    .child(mono(name.clone()).flex_shrink_0().text_color(c.text))
                    .children(member.map(|member| ui::badge(member.runtime.clone())))
                    .child(
                        ui::text(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(c.muted),
                            &t.text.meta,
                        )
                        .child(member.map(folder).unwrap_or_default()),
                    )
                    .children(tag(machine))
                    .children(ui::glance_counts(counts))
                    .child(ui::text(
                        ui::agent_status_text(&status).flex_shrink_0(),
                        &t.text.meta,
                    ))
                    .child(ui::choice_key(key))
                    .into_any_element()
            }
            agent_groups::Row::Member {
                name,
                short,
                state,
                machine,
                ..
            } => child()
                // A stale state is not shown as current, as the panel does.
                .child(ui::agent_symbol(ui::shown_state(
                    *state,
                    self.agents.get(name).stale,
                )))
                .child(mono(short.clone()).flex_shrink_0().text_color(c.text))
                .children(
                    self.member(name)
                        .map(|member| ui::badge(member.runtime.clone())),
                )
                .children(tag(machine))
                .child(fill())
                // Only a member's row says what Enter does, as the board shows.
                // As the hint bar says it.
                .children(selected.then(|| {
                    attach_hint(if self.remote(name).is_some() {
                        "open remote terminal"
                    } else {
                        "attach here"
                    })
                }))
                .into_any_element(),
            // One that cannot be chosen looks it: no pointer, no hover.
            agent_groups::Row::Review { review, .. } => (if choosable {
                child()
            } else {
                quiet_frame(id.clone())
            })
            .children(symbol)
            .children(label.map(filling))
            .child(meta(
                match &review.effort {
                    Some(effort) => format!("review · {effort}"),
                    None => "review".to_owned(),
                },
                c.muted,
            ))
            .into_any_element(),
            agent_groups::Row::Browser { .. } => child()
                .children(symbol)
                .children(label.map(filling))
                .child(meta("browser", c.muted))
                .into_any_element(),
            agent_groups::Row::Swarm {
                initiator,
                name,
                members,
                listed,
                expanded,
            } => {
                let key = agent_groups::swarm_key(initiator, name);
                // A filter never folds, so it shows no chevron.
                let right = if *expanded || *listed == 0 || !self.query.is_empty() {
                    meta("open all as a tab", c.muted).into_any_element()
                } else {
                    chevron(format!("new-pane-swarm-{}-{key}", self.pane), false)
                        .on_click(cx.listener(move |chooser, _, _, cx| {
                            cx.stop_propagation();
                            if chooser.query.is_empty() {
                                chooser.collapsed_swarms.remove(&key);
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                };
                child()
                    .children(symbol)
                    .children(label.map(|label| label.flex_shrink_0()))
                    .child(meta(
                        match members.len() {
                            1 => "swarm · 1 agent".to_owned(),
                            count => format!("swarm · {count} agents"),
                        },
                        c.muted,
                    ))
                    .child(fill())
                    .child(right)
                    .into_any_element()
            }
        }
    }

    /// A connection or request of this profile, or with none, all of them.
    fn connection_row(
        &self,
        id: SharedString,
        selection: Option<&Selection>,
        key: String,
        selected: bool,
    ) -> gpui::Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let row = ui::choice_frame(id, selected).child(chevron_slot());
        let entry = selection.and_then(|selection| {
            self.connections
                .iter()
                .find(|entry| entry.selection == *selection)
        });
        let Some(entry) = entry else {
            return row
                .child(ui::icon_slot("link-2"))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(c.text)
                        .child(ALL_CONNECTIONS),
                )
                .child(
                    ui::text(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(c.text_2),
                        &t.text.meta,
                    )
                    .child("Requests, review gates and this profile's connection ID"),
                )
                .child(ui::choice_key(key));
        };
        row.child(peer_rows::peer_mark(
            entry.name.as_deref(),
            t.size.agent_symbol,
        ))
        .child(
            div()
                .flex_shrink_0()
                .text_color(peer_rows::name_color(entry.name.as_deref()))
                .child(peer_rows::display_name(entry.name.as_deref()).to_owned()),
        )
        .children(entry.connected.then(|| meta("connected", c.ok)))
        .child(div().flex_1().min_w_0())
        .children(
            entry
                .badge
                .as_ref()
                .map(|(label, badge)| peer_rows::badge(label, *badge)),
        )
        .child(ui::choice_key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(entries: &[(&str, AgentState)]) -> Vec<(String, AgentState)> {
        entries
            .iter()
            .map(|(name, state)| ((*name).to_owned(), *state))
            .collect()
    }

    fn fix_swarm() -> Swarm {
        Swarm {
            name: "fix".into(),
            initiator: "lead".into(),
            members: vec!["fix-coordinator".into(), "fix-coder".into()],
        }
    }

    fn started(name: &str) -> agent_groups::Started {
        let review = |reviewer: &str, view: Option<&str>| agent_groups::Review {
            reviewer: reviewer.into(),
            effort: Some("high".into()),
            running: true,
            view: view.map(str::to_owned),
        };
        match name {
            "lead" => agent_groups::Started {
                reviews: vec![
                    review("unwatched", None),
                    review("watched", Some("run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60")),
                ],
                browser: None,
            },
            "helper" => agent_groups::Started {
                reviews: Vec::new(),
                browser: Some("example.com".into()),
            },
            _ => agent_groups::Started::default(),
        }
    }

    fn entries() -> Vec<(String, AgentState)> {
        listed(&[
            ("lead", AgentState::Working),
            ("fix-coordinator", AgentState::Working),
            ("fix-coder", AgentState::Idle),
            ("helper", AgentState::Idle),
            ("solo-coder", AgentState::Idle),
        ])
    }

    /// The list's rows for these folds, every agent here.
    fn shown(expanded: &[&str], collapsed: &[&str]) -> Vec<Row> {
        let expanded = expanded.iter().map(|name| (*name).to_owned()).collect();
        let collapsed = collapsed.iter().map(|key| (*key).to_owned()).collect();
        agent_rows(
            &entries(),
            &[fix_swarm()],
            started,
            |_| None,
            (&expanded, &collapsed),
            None,
        )
        .into_iter()
        .map(|row| {
            let choosable = choosable(&row, |_| true);
            Row::Tree(Box::new(row), choosable, String::new())
        })
        .collect()
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter().map(row_key).collect()
    }

    #[test]
    fn right_opens_an_agent_down_to_its_swarm_members_and_left_climbs_back() {
        let folded = shown(&[], &[]);
        assert_eq!(
            keys(&folded),
            ["agent:lead:", "agent:helper:", "agent:solo-coder:"]
        );
        assert_eq!(
            step(&folded, "agent:lead:", true),
            Step::Open("lead".into())
        );
        assert_eq!(step(&folded, "agent:lead:", false), Step::Nothing);
        // Nothing under it: no chevron, and → does nothing.
        assert_eq!(step(&folded, "agent:solo-coder:", true), Step::Nothing);

        let open = shown(&["lead"], &[]);
        assert_eq!(
            keys(&open),
            [
                "agent:lead:",
                "review:/lead/0",
                "review:/run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60",
                "swarm:lead/fix",
                "member:fix/fix-coordinator:",
                "member:fix/fix-coder:",
                "agent:helper:",
                "agent:solo-coder:",
            ]
        );
        // The member is a row Enter can reach.
        let choices: Vec<String> = open
            .iter()
            .filter(|row| NewPane::selectable(row))
            .map(row_key)
            .collect();
        assert!(choices.contains(&"member:fix/fix-coder:".to_owned()));
        assert_eq!(
            step(&open, "member:fix/fix-coder:", false),
            Step::Select("swarm:lead/fix".into())
        );
        assert_eq!(step(&open, "member:fix/fix-coder:", true), Step::Nothing);
        assert_eq!(
            step(&open, "swarm:lead/fix", false),
            Step::FoldSwarm(agent_groups::swarm_key("lead", "fix"))
        );
        assert_eq!(
            step(&open, "review:/run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60", false),
            Step::Select("agent:lead:".into())
        );
        assert_eq!(
            step(&open, "agent:lead:", false),
            Step::Close("lead".into())
        );

        // A folded swarm opens with →; ← then goes up to its agent.
        let swarm_folded = shown(&["lead"], &["lead/fix"]);
        assert!(!keys(&swarm_folded).contains(&"member:fix/fix-coder:".to_owned()));
        assert_eq!(
            step(&swarm_folded, "swarm:lead/fix", true),
            Step::OpenSwarm("lead/fix".into())
        );
        assert_eq!(
            step(&swarm_folded, "swarm:lead/fix", false),
            Step::Select("agent:lead:".into())
        );
    }

    #[test]
    fn a_review_without_a_view_here_cannot_be_chosen() {
        let open = shown(&["lead"], &[]);
        let (unwatched, watched) = (&open[1], &open[2]);
        assert!(!NewPane::selectable(unwatched) && !NewPane::numbered(unwatched));
        assert!(NewPane::selectable(watched));
        // Nor one whose agent runs on another machine: its view is there.
        let Row::Tree(row, ..) = watched else {
            unreachable!()
        };
        assert!(matches!(row.as_ref(), agent_groups::Row::Review { .. }));
        assert!(choosable(row, |agent| agent == "lead"));
        assert!(!choosable(row, |_| false));
    }

    #[test]
    fn a_filter_shows_matching_rows_under_their_agents_and_keeps_the_folds() {
        let (expanded, collapsed) = (HashSet::new(), HashSet::new());
        let filtered = |query: &str| {
            let matches =
                |row: &agent_groups::Row| tree_matches(row, |_| None, |text| text.contains(query));
            agent_rows(
                &entries(),
                &[fix_swarm()],
                started,
                |_| None,
                (&expanded, &collapsed),
                Some(&matches),
            )
        };
        let coder = filtered("coder");
        assert_eq!(
            coder
                .iter()
                .map(|row| tree_key(row, ""))
                .collect::<Vec<_>>(),
            [
                "agent:lead:",
                "swarm:lead/fix",
                "member:fix/fix-coder:",
                "agent:solo-coder:"
            ]
        );
        // Shown open while something under them shows; nothing under the
        // agent that matched by itself.
        assert!(matches!(
            &coder[0],
            agent_groups::Row::Agent { expanded: true, .. }
        ));
        assert!(matches!(
            &coder[1],
            agent_groups::Row::Swarm { expanded: true, .. }
        ));
        assert!(matches!(
            &coder[3],
            agent_groups::Row::Agent {
                expanded: false,
                ..
            }
        ));
        // A browser by its host, a review by its reviewer.
        let host = filtered("example");
        assert!(
            matches!(&host[..], [agent_groups::Row::Agent { name, .. }, agent_groups::Row::Browser { .. }] if name == "helper")
        );
        assert_eq!(filtered("watched").len(), 3);
        assert!(expanded.is_empty() && collapsed.is_empty());
    }

    #[test]
    fn number_keys_reach_top_level_rows_only_agents_then_connections_then_sessions() {
        let entry = |request: &str, name: Option<&str>| peer_rows::Entry {
            selection: Selection::Request(request.into()),
            name: name.map(str::to_owned),
            peer_id: "ab".repeat(32),
            connected: false,
            badge: None,
            needs: false,
        };
        let connections = [entry("r1", Some("Example")), entry("r2", None)];
        assert_eq!(
            keys(&connection_rows(&connections, |_| true)),
            ["request:r1", "request:r2", "connections:all"]
        );
        // An unnamed connection matches by the name it shows.
        assert_eq!(
            keys(&connection_rows(&connections, |text| text.contains("Unnamed"))),
            ["request:r2"]
        );
        let build = || {
            let mut rows = vec![Row::Heading("BACKBONE AGENTS", "")];
            rows.extend(shown(&["lead"], &[]));
            rows.push(Row::Heading("CONNECTIONS", ""));
            rows.extend(connection_rows(&connections[..1], |_| true));
            rows.push(Row::Heading("OTHER TMUX SESSIONS", ""));
            rows.push(Row::Session("scratch".into()));
            rows
        };
        let rows = build();
        let presented: Vec<String> = rows
            .iter()
            .filter(|row| NewPane::numbered(row))
            .map(row_key)
            .collect();
        assert_eq!(
            presented,
            [
                "agent:lead:",
                "agent:helper:",
                "agent:solo-coder:",
                "request:r1",
                "connections:all",
                "scratch"
            ]
        );
        let choices = || {
            build()
                .into_iter()
                .filter(NewPane::selectable)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            row_key(&numbered_row(&presented, 4, choices()).unwrap()),
            "request:r1"
        );
        assert_eq!(
            row_key(&numbered_row(&presented, 6, choices()).unwrap()),
            "scratch"
        );
    }

    #[test]
    fn the_folder_text_leaves_the_machine_to_its_tag() {
        let member = Member {
            name: "remote".into(),
            repo: "example/project".into(),
            machine: Some(agent_interface_roster::Machine {
                id: "b".repeat(64),
                name: "build-box".into(),
            }),
            ..Member::default()
        };
        assert_eq!(folder(&member), "example/project");
        let tagged = Member {
            repo: String::new(),
            tags: vec!["writer".into(), "research".into()],
            ..member.clone()
        };
        assert_eq!(folder(&tagged), "writer · research");
        // The tag shows only another machine, and members carry theirs.
        let tag = |name: &str| {
            (name == "fix-coder").then(|| MachineHeader {
                id: "b".repeat(64),
                name: "build-box".into(),
                presence: agent_groups::MachinePresence::Online,
            })
        };
        let rows = agent_rows(
            &entries(),
            &[fix_swarm()],
            started,
            tag,
            (&HashSet::from(["lead".to_owned()]), &HashSet::new()),
            None,
        );
        assert!(rows.iter().any(|row| matches!(row, agent_groups::Row::Member { name, machine: Some(m), .. } if name == "fix-coder" && m.name == "build-box")));
        assert!(matches!(
            &rows[0],
            agent_groups::Row::Agent { machine: None, .. }
        ));
        // The filter finds a member by its machine's tag.
        let on_box: Vec<_> = rows
            .iter()
            .filter(|row| tree_matches(row, |_| None, |text| text.contains("build-box")))
            .collect();
        assert!(
            matches!(&on_box[..], [agent_groups::Row::Member { name, .. }] if name == "fix-coder")
        );
    }

    #[test]
    fn arrows_fold_only_the_agents_list_with_an_empty_filter() {
        assert_eq!(folding(Mode::Sessions, "", "right"), Some(true));
        assert_eq!(folding(Mode::Sessions, "", "left"), Some(false));
        // With text, the filter's caret moves; elsewhere they do nothing.
        assert_eq!(folding(Mode::Sessions, "co", "right"), None);
        assert_eq!(folding(Mode::Views, "", "left"), None);
        assert_eq!(folding(Mode::Sessions, "", "down"), None);
    }

    #[test]
    fn a_step_changes_the_list_folds_and_selection_it_names() {
        let (mut expanded, mut collapsed, mut selected) = (HashSet::new(), HashSet::new(), None);
        let mut run = |step| apply(step, &mut expanded, &mut collapsed, &mut selected);
        assert!(run(Step::Open("lead".into())));
        assert!(!run(Step::Open("lead".into())));
        assert!(run(Step::FoldSwarm("lead/fix".into())));
        assert!(run(Step::Select("swarm:lead/fix".into())));
        assert!(!run(Step::Nothing));
        assert_eq!(expanded, HashSet::from(["lead".to_owned()]));
        assert_eq!(collapsed, HashSet::from(["lead/fix".to_owned()]));
        assert_eq!(selected.as_deref(), Some("swarm:lead/fix"));
        let mut run = |step| apply(step, &mut expanded, &mut collapsed, &mut selected);
        assert!(run(Step::OpenSwarm("lead/fix".into())));
        assert!(run(Step::Close("lead".into())));
        assert!(expanded.is_empty() && collapsed.is_empty());
    }

    #[test]
    fn the_first_choice_waits_until_every_read_has_landed() {
        // (a) Sessions and team read, the roster with the swarms not yet:
        // a member is still listed alone and the connections are there.
        assert!(reading(Mode::Sessions, false, false, Some(false)));
        // (b) The team and roster read, the sessions not yet: only remote
        // agents are listed, some of them members of a local initiator.
        assert!(reading(Mode::Sessions, true, false, Some(true)));
        assert!(reading(Mode::Sessions, false, true, Some(true)));
        assert!(!reading(Mode::Sessions, false, false, Some(true)));
        // Without a profile there is no roster to wait for.
        assert!(!reading(Mode::Sessions, false, false, None));
        assert!(reading(Mode::Agents, false, true, Some(true)));
        assert!(!reading(Mode::Views, true, true, Some(false)));

        // While reading, no row of any kind is chosen for the person.
        let mut rows = shown(&[], &[]);
        rows.push(Row::Heading("CONNECTIONS", ""));
        rows.push(Row::Connection(None));
        rows.push(Row::Session("scratch".into()));
        let (mut selected, mut disarmed) = (None, false);
        reconcile_selection(&rows, &mut selected, &mut disarmed, true, |_| true);
        assert!(selected.is_none() && !disarmed);
        reconcile_selection(&rows[3..], &mut selected, &mut disarmed, true, |_| true);
        assert!(selected.is_none() && !disarmed);
        // Read, the first row is chosen: the agent most in need.
        reconcile_selection(&rows, &mut selected, &mut disarmed, false, |_| true);
        assert_eq!(selected.as_deref(), Some("agent:lead:"));
        // A list of connections alone still offers its first.
        let (mut selected, mut disarmed) = (None, false);
        reconcile_selection(&rows[4..6], &mut selected, &mut disarmed, false, |_| true);
        assert_eq!(selected.as_deref(), Some("connections:all"));
    }

    #[test]
    fn under_a_filter_the_first_choice_is_the_match_not_the_rows_above_it() {
        let (expanded, collapsed) = (HashSet::new(), HashSet::new());
        let matches = |row: &agent_groups::Row| {
            tree_matches(row, |_| None, |text| text.contains("fix-coder"))
        };
        let rows: Vec<Row> = agent_rows(
            &entries(),
            &[fix_swarm()],
            started,
            |_| None,
            (&expanded, &collapsed),
            Some(&matches),
        )
        .into_iter()
        .map(|row| Row::Tree(Box::new(row), true, String::new()))
        .collect();
        assert_eq!(
            keys(&rows),
            ["agent:lead:", "swarm:lead/fix", "member:fix/fix-coder:"]
        );
        let own = |row: &Row| matches!(row, Row::Tree(line, ..) if matches(line));
        let (mut selected, mut disarmed) = (None, false);
        reconcile_selection(&rows, &mut selected, &mut disarmed, false, own);
        assert_eq!(selected.as_deref(), Some("member:fix/fix-coder:"));
    }

    #[test]
    fn a_same_name_agent_on_another_machine_is_another_row() {
        let agent = || {
            Box::new(agent_groups::Row::Agent {
                name: "worker".into(),
                state: AgentState::Idle,
                machine: None,
                counts: agent_groups::Counts::default(),
                children: false,
                expanded: false,
            })
        };
        let here = Row::Tree(agent(), true, "a".repeat(64));
        let there = Row::Tree(agent(), true, "b".repeat(64));
        assert_ne!(row_key(&here), row_key(&there));
        let presented = vec![row_key(&here)];
        assert!(numbered_row(&presented, 1, vec![there]).is_none());
    }

    #[test]
    fn same_name_replacement_cannot_inherit_displayed_agent_selection() {
        let (_dir, connection, server) = crate::roster::content::tests::provider("a", vec![]);
        let origin = Origin::Bound(connection);
        let mut member = Member {
            name: "worker".into(),
            registration_id: Some("1234567890abcdef1234567890abcdef".into()),
            ..Member::default()
        };
        let original = origin.agent_view(&member).unwrap();
        let key = row_key(&Row::Pick(original.clone()));
        member.registration_id = Some("abcdef1234567890abcdef1234567890".into());
        let replacement = origin.agent_view(&member).unwrap();
        assert!(
            numbered_row(
                std::slice::from_ref(&key),
                1,
                vec![Row::Pick(replacement.clone())]
            )
            .is_none()
        );
        let mut selected = Some(key);
        let mut disarmed = false;
        reconcile_selection(
            &[Row::Pick(replacement)],
            &mut selected,
            &mut disarmed,
            false,
            |_| true,
        );
        assert!(selected.is_none() && disarmed);
        member.registration_id = None;
        assert!(origin.agent_view(&member).is_none());
        member.registration_id = Some("invalid".into());
        assert!(origin.agent_view(&member).is_none());
        // A registration a profile accepts still opens no agent view without one.
        member.registration_id = Some("abcdef1234567890abcdef1234567890".into());
        assert!(origin.agent_view(&member).is_some());
        assert!(
            Origin::Standalone { socket: None }
                .agent_view(&member)
                .is_none()
        );
        server.join().unwrap();
    }

    #[test]
    fn numbered_selection_keeps_displayed_identity_across_pending_refreshes() {
        let presented = vec!["alpha".into(), "beta".into()];
        assert!(numbered_row(&presented, 1, vec![Row::Session("beta".into())]).is_none());
        let reordered = vec![Row::Session("beta".into()), Row::Session("alpha".into())];
        assert_eq!(
            row_key(&numbered_row(&presented, 1, reordered).unwrap()),
            "alpha"
        );
        assert_eq!(
            row_key(&numbered_row(&presented, 2, vec![Row::Session("beta".into())]).unwrap()),
            "beta"
        );
        assert!(numbered_row(&presented, 0, vec![]).is_none());
        assert!(numbered_row(&presented, 3, vec![Row::Session("gamma".into())]).is_none());
    }

    #[test]
    fn tab_moves_through_sessions_sections_and_views_both_ways() {
        let order = [Mode::Sessions, Mode::Reports, Mode::Views];
        for (at, mode) in order.into_iter().enumerate() {
            assert_eq!(cycle(mode, false), order[(at + 1) % 3]);
            assert_eq!(cycle(mode, true), order[(at + 2) % 3]);
        }
        assert_eq!(cycle(Mode::Agents, false), Mode::Sessions);
    }

    #[test]
    fn row_reconciliation_keeps_a_choice_made_while_reading() {
        let rows = vec![Row::Heading("SESSIONS", ""), Row::Session("worker".into())];
        let (mut selected, mut disarmed) = (None, false);
        reconcile_selection(&rows, &mut selected, &mut disarmed, true, |_| true);
        assert!(selected.is_none());
        // The person's own choice, made while reading, stands.
        selected = Some("worker".into());
        // A reveal may briefly have no rows while fresh data is being read.
        reconcile_selection(&[], &mut selected, &mut disarmed, true, |_| true);
        assert_eq!(selected.as_deref(), Some("worker"));
        let reordered = vec![Row::Session("other".into()), Row::Session("worker".into())];
        reconcile_selection(&reordered, &mut selected, &mut disarmed, false, |_| true);
        assert_eq!(selected.as_deref(), Some("worker"));
        reconcile_selection(&reordered[..1], &mut selected, &mut disarmed, false, |_| {
            true
        });
        assert!(selected.is_none());
        assert!(disarmed);
        reconcile_selection(&rows, &mut selected, &mut disarmed, false, |_| true);
        assert!(selected.is_none());
    }
}
