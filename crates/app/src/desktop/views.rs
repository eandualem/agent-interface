//! Native information views and local HTML artifacts shown beside terminals.
//!
//! A [`ViewPane`] exists while its pane is shown, and a hidden saved pane
//! keeps it while it holds conversations or Sheets. It polls its data on an
//! I/O worker only while visible (`source::Source`). Each view draws its
//! data in its own module from the shared pieces in `components`, following
//! the design boards. Every view stays inside its pane: choosing a skill or a
//! policy opens it in the same pane through `reader`, with a way back. Backbone
//! supplies record reads; a separate native composer sends explicit feedback.
//! Artifacts have their own local catalog and an isolated system-webview renderer rather than a Backbone worker.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

mod bound_agent;

use agent_interface_core::{AgentSection, View, ViewKind};
use agent_interface_roster::views::{self, Data, Range, Request};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, SharedString, Subscription, Task, Window, div, prelude::*, px,
};

use super::{
    components::{self as ui, mono},
    design::{colors, tokens},
    name::{NameEvent, NameField},
};
use agent_interface_roster::profiles::Connection;
use source::Source;

mod activity;
mod additions;
mod agent;
pub(super) mod artifact_web;
mod artifacts;
mod bound_policies;
mod bound_skills;
mod clock;
mod connections;
mod conversation;
mod documents;
mod interactions;
mod markdown;
mod plans;
mod policies;
mod reader;
mod reading;
mod reports;
mod seen;
mod selection;
mod sheets;
mod skills;
pub(super) mod source;
mod tags;
mod team;
mod usage;

/// A kind of view's icon and name.
pub(super) struct About {
    pub icon: &'static str,
    pub label: &'static str,
}

/// The icon and name of each kind.
pub(super) fn about(kind: ViewKind) -> About {
    let (icon, label) = match kind {
        ViewKind::Skills => ("diamond", "Skills"),
        ViewKind::Agent => ("user", "Agent"),
        ViewKind::Policies => ("align-left", "Policies"),
        ViewKind::Usage => ("chart-line", "Usage"),
        ViewKind::Activity => ("activity", "Activity"),
        ViewKind::Team => ("bot", "Team"),
        ViewKind::Connections => ("link-2", "Connections"),
        ViewKind::Tags => ("hash", "Tags"),
        ViewKind::Reports => ("message-square", "Reports & updates"),
        ViewKind::Artifacts => ("layout-grid", "Artifacts"),
    };
    About { icon, label }
}

/// What a view asks its owner to do.
pub(super) enum ViewEvent {
    /// Open this view in a new pane beside this one.
    OpenBeside(View),
    /// Open this run's view, a session on the profile's tmux server, read-only
    /// in a new pane beside this one.
    RunView(String),
    /// An embedded document received a pointer press; select its owning pane.
    Focus,
}

/// How the team view groups agents.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Grouping {
    /// By workspace folder, coordinators first.
    Folder,
    Tag,
    Swarm,
    Machine,
}

/// Lifecycle action triggered from a Team roster row.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TeamAgentAction {
    Start { resume: bool },
    Stop,
    Restart { resume: bool },
}

/// How the usage view groups its rows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageBy {
    Agent,
    Model,
    Cli,
}

/// Which records the activity view lists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Records {
    All,
    Deliveries,
    GitHub,
    Held,
}

/// Which kind of Reports & updates section a pane shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Shown {
    /// A record feed: waiting on you, updates, answers or documents.
    Records,
    Plans,
    Interactions,
    Sheets,
}

/// A Reports & updates section a new pane opens on, in the bar's order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Waiting,
    Plans,
    Interactions,
    Updates,
    Responses,
    Documents,
    Sheets,
}

/// Something a list can open in the reader, by the provider's path for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// A skill, whose SKILL.md snapshot is read when opened.
    Skill(String),
    /// A policy, by its listed path. In Policies, a catalog policy's text is read
    /// from its snapshot and a base or swarm layer shows its listed text; an
    /// agent's brief shows the text Backbone listed in it.
    Policy(String),
}

/// What the reader shows in place of a list.
pub(super) struct Open {
    pub target: Target,
    bound_skill: Option<bound_skills::Open>,
    bound_policy: Option<bound_policies::Open>,
    /// The text the reader shows: a snapshot's once read, or a policy's listed
    /// text.
    pub text: Option<Result<std::sync::Arc<str>, &'static str>>,
}

/// What a Reports pane reads: its open record, or else its feed.
fn reports_request(reports: &reports::State) -> Request {
    match &reports.open {
        // A correction asked for by number reads only that answer.
        _ if let Some(id) = reports.pending_correction() => Request::Response(id),
        Some(item) if matches!(item.content, views::ReportContent::Document(_)) => {
            Request::Document(item.id)
        }
        Some(item) if matches!(item.content, views::ReportContent::Response { .. }) => {
            Request::Response(item.id)
        }
        _ => Request::Reports(reports.query.clone(), reports.agent.clone()),
    }
}

/// Close the open record and drop a Document's page and its work. A read
/// for that record then no longer matches the pane's request and is dropped.
fn leave(documents: &mut documents::State, reports: &mut reports::State) {
    documents.clear();
    reports.open = None;
}

/// The folders of a window's workspaces, read when a Document is opened.
pub(super) type Folders = Rc<dyn Fn(&gpui::App) -> Vec<PathBuf>>;

/// One view in one pane, with its newest data and what the person chose in it.
pub(super) struct ViewPane {
    agents: crate::desktop::agent_status::LiveAgents,
    /// Where a Document's code may be checked out, with the agents' folders.
    folders: Folders,
    pub view: View,
    source: Option<Source>,
    connections: Option<Entity<connections::Pane>>,
    /// Immutable origin for retained content and every operation it opens.
    connection: Option<Connection>,
    data: Option<Data>,
    /// When `data` was read.
    read_at: Option<std::time::Instant>,
    error: Option<&'static str>,
    range: Range,
    grouping: Grouping,
    team_notice: Option<String>,
    team_actions_in_flight: HashSet<String>,
    usage_by: UsageBy,
    cost: bool,
    expanded: HashSet<String>,
    records: Records,
    /// The shown section of an agent's view.
    section: AgentSection,
    query: String,
    search: Option<Entity<NameField>>,
    /// What is open in place of the list, found by its file so a refresh
    /// never swaps it for another.
    open: Option<Open>,
    /// Whether the reader's Markdown was just copied.
    copied: bool,
    /// The tag whose agents the tags view lists in place of its cards.
    carriers_of: Option<String>,
    selection: RefCell<selection::Selection>,
    reports: reports::State,
    additions: additions::State,
    /// Conversations by provider, author and record, kept while the pane exists.
    conversations: HashMap<String, Entity<conversation::Conversation>>,
    /// The open record's conversation, for its reader to place.
    conversation: Option<Entity<conversation::Conversation>>,
    /// Whether this pane is visible; its conversations read Backbone only then.
    visible: Rc<Cell<bool>>,
    documents: documents::State,
    artifacts: Option<Entity<artifacts::ArtifactPane>>,
    sheets: Option<Entity<sheets::Sheets>>,
    plans: Option<Entity<plans::Plans>>,
    interactions: Option<Entity<interactions::Interactions>>,
    bound_agent: Option<Entity<bound_agent::Card>>,
    /// The Reports & updates section shown when it is not a record feed.
    pub(super) shown: Shown,
    _updates: Option<Task<()>>,
    _search: Vec<Subscription>,
}

impl EventEmitter<ViewEvent> for ViewPane {}

impl ViewPane {
    /// Keep drafts, threads and in-flight delivery while this saved pane is hidden.
    pub fn has_conversations(&self) -> bool {
        !self.conversations.is_empty()
            || self.sheets.is_some()
            || self.interactions.is_some()
            || self.connections.is_some()
    }

    /// Native child views must repaint when an overlay stops occluding them.
    pub fn paints_webview(&self) -> bool {
        self.artifacts.is_some() || documents::selected(self).is_some()
    }

    pub fn set_visible(&self, visible: bool, cx: &gpui::App) {
        self.visible.set(visible);
        if !visible {
            self.documents.surface.borrow_mut().clear();
        }
        if let Some(source) = &self.source {
            source.set_active(visible);
        }
        if let Some(artifacts) = &self.artifacts {
            artifacts.read(cx).set_visible(visible);
        }
    }

    pub fn occlude_web(&self, occluded: bool, cx: &gpui::App) {
        self.documents.surface.borrow_mut().occlude(occluded);
        if let Some(artifacts) = &self.artifacts {
            artifacts.read(cx).occlude(occluded);
        }
    }

    pub fn focus_native(&self, cx: &gpui::App) {
        self.documents.surface.borrow().focus_parent();
        if let Some(artifacts) = &self.artifacts {
            artifacts.read(cx).focus_parent();
        }
    }

    /// A view in a workspace without a profile: only the local artifact
    /// catalog has anything to show there.
    pub fn new(
        view: View,
        directory: Option<PathBuf>,
        folders: Folders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with(
            view,
            directory,
            Default::default(),
            folders,
            None,
            window,
            cx,
        )
    }

    /// A view in a workspace with a profile, reading through its verified
    /// connection.
    pub(super) fn pick_peer(
        &self,
        selection: super::peer_inbox::Selection,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = &self.connections {
            pane.update(cx, |p, cx| p.pick_peer(selection, cx));
        }
    }
    /// Tell a Connections pane which workspace and profile it is in, for
    /// its saved peer list choice and this profile's name.
    pub(super) fn place_connections(
        &self,
        profile: Option<&str>,
        workspace: &str,
        name: Option<&str>,
        cx: &mut gpui::App,
    ) {
        if let Some(pane) = &self.connections {
            pane.update(cx, |pane, cx| {
                if pane.place(profile, workspace, name) {
                    cx.notify();
                }
            });
        }
    }

    pub(super) fn connection_attention(&self, cx: &gpui::App) -> bool {
        self.connections
            .as_ref()
            .is_some_and(|p| p.read(cx).needs_review(cx))
    }

    pub fn new_bound(
        view: View,
        connection: Connection,
        directory: Option<PathBuf>,
        agents: crate::desktop::agent_status::LiveAgents,
        folders: Folders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with(
            view,
            directory,
            agents,
            folders,
            Some(connection),
            window,
            cx,
        )
    }

    fn new_with(
        view: View,
        directory: Option<PathBuf>,
        agents: crate::desktop::agent_status::LiveAgents,
        folders: Folders,
        connection: Option<Connection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            agents,
            folders,
            source: None,
            connections: None,
            connection,
            data: None,
            read_at: None,
            error: None,
            range: Range::Day,
            grouping: Grouping::Folder,
            team_notice: None,
            team_actions_in_flight: HashSet::new(),
            usage_by: UsageBy::Agent,
            cost: false,
            expanded: HashSet::new(),
            records: Records::All,
            // The agent card's Opened event sets the section its reader lists.
            section: AgentSection::default(),
            query: String::new(),
            search: None,
            open: None,
            copied: false,
            carriers_of: None,
            selection: RefCell::default(),
            reports: reports::State::default(),
            additions: additions::State::default(),
            conversations: HashMap::new(),
            conversation: None,
            visible: Rc::new(Cell::new(true)),
            documents: documents::State::default(),
            artifacts: None,
            sheets: None,
            plans: None,
            interactions: None,
            bound_agent: None,
            shown: Shown::Records,
            _updates: None,
            _search: Vec::new(),
            view,
        };
        if pane.connection.is_none() && pane.view.registration_id.is_some() {
            pane.error = Some("This saved agent view requires its original profile.");
            return pane;
        }
        if let Some(connection) = &pane.connection
            && !pane.agents.belongs_to(connection.identity())
        {
            pane.error = Some("This view is unavailable for the original profile.");
            return pane;
        }
        if pane.view.kind == ViewKind::Agent
            && let Some(connection) = &pane.connection
        {
            if !pane.view.is_valid() || pane.view.registration_id.is_none() {
                pane.error = Some("Reopen this agent from the profile's Team view.");
                return pane;
            }
            let connection = connection.clone();
            let visible = pane.visible.clone();
            let agents = pane.agents.clone();
            let card =
                cx.new(|cx| bound_agent::Card::new(connection, agents, &pane.view, visible, cx));
            // A chosen skill or policy opens in this pane's reader, in place of
            // the card until the person goes back.
            pane._search.push(
                cx.subscribe(&card, |pane, _, opened: &bound_agent::Opened, cx| {
                    pane.section = opened.section;
                    pane.data = Some(Data::Agent(Box::new(opened.agent.clone())));
                    pane.open(opened.target.clone(), cx);
                }),
            );
            pane._search.push(
                cx.subscribe(&card, |_, _, view: &bound_agent::RunView, cx| {
                    cx.emit(ViewEvent::RunView(view.0.clone()))
                }),
            );
            pane.bound_agent = Some(card);
            return pane;
        }
        if pane.view.kind == ViewKind::Artifacts {
            let artifacts = artifacts::create(directory, cx);
            pane._search
                .push(cx.subscribe(&artifacts, |_, _, _: &artifacts::Focus, cx| {
                    cx.emit(ViewEvent::Focus)
                }));
            pane.artifacts = Some(artifacts);
            return pane;
        }
        let Some(connection) = pane.connection.clone() else {
            pane.error = Some("Choose a profile for this workspace to show this view.");
            return pane;
        };
        if pane.view.kind == ViewKind::Connections {
            pane.connections = Some(cx.new(|cx| connections::Pane::new(connection, window, cx)));
            return pane;
        }
        if matches!(pane.view.kind, ViewKind::Skills | ViewKind::Reports) {
            let field = cx.new(|cx| NameField::filter(window, cx));
            pane._search
                .push(cx.subscribe(&field, |pane, _, event, cx| {
                    if let NameEvent::Changed(query) = event {
                        pane.query = query.clone();
                        cx.notify();
                    }
                }));
            pane.search = Some(field);
        }
        // Backbone collects usage about once a minute, and a library gathers
        // many agents' previews and changes rarely; everything else is live.
        let every = Duration::from_secs(match pane.view.kind {
            ViewKind::Usage => 15,
            ViewKind::Skills | ViewKind::Policies | ViewKind::Tags => 30,
            _ => 3,
        });
        let (wake, mut woken) = mpsc::unbounded();
        match Source::start(&connection, pane.request(), every, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(source) => pane.source = Some(source),
            Err(_) => pane.error = Some(views::UNAVAILABLE),
        }
        pane._updates = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                let updated = this.update(cx, |pane, cx| {
                    if let Some((request, update)) =
                        pane.source.as_ref().and_then(Source::take_update)
                    {
                        // A read for an earlier request, such as another
                        // usage range, must not be shown under the new one.
                        if request != pane.request() {
                            return;
                        }
                        match update {
                            Ok(data) => {
                                if let Data::Document(document) = &data {
                                    pane.documents.error = None;
                                    documents::serve(pane, document, cx);
                                }
                                if let Data::Reports(page) = &data {
                                    pane.reports.observe(page);
                                }
                                pane.error = None;
                                pane.data = Some(data);
                                pane.read_at = Some(std::time::Instant::now());
                            }
                            // Keep the last data and say it is stale.
                            Err(error) => pane.error = Some(error),
                        }
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
        pane
    }

    fn request(&self) -> Request {
        match self.view.kind {
            ViewKind::Team => Request::Team,
            ViewKind::Usage => Request::Usage(self.range),
            ViewKind::Activity => Request::Activity,
            ViewKind::Skills => Request::Skills,
            ViewKind::Policies => Request::Policies,
            ViewKind::Tags => Request::Tags,
            ViewKind::Reports => reports_request(&self.reports),
            ViewKind::Connections => unreachable!("connections do not use the team content source"),
            ViewKind::Artifacts => unreachable!("artifacts have no Backbone source"),
            // A profile's agent card reads through its own connection.
            ViewKind::Agent => unreachable!("agent cards have no view source"),
        }
    }

    /// Open on `section`, as the new-pane chooser asks; the pane does not
    /// remember it.
    pub(super) fn open_section(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = match section {
            Section::Plans => return self.show_plans(cx),
            Section::Interactions => return self.show_interactions(window, cx),
            Section::Sheets => return self.show_sheets(window, cx),
            Section::Waiting => views::ReportQuery::Latest(None),
            Section::Updates => views::ReportQuery::Recent(None),
            Section::Responses => views::ReportQuery::Responses(None),
            Section::Documents => views::ReportQuery::Documents(None, None),
        };
        self.show_records(query, cx);
    }

    /// Show a record feed from its first page.
    fn show_records(&mut self, query: views::ReportQuery, cx: &mut Context<Self>) {
        self.plans_shown(false, cx);
        self.interactions_shown(false, cx);
        self.shown = Shown::Records;
        self.reports.query = query;
        self.reports.previous.clear();
        self.leave_reader(cx);
    }

    /// Close the open record, and with it a Document's page and its work,
    /// as every section switch does, and read for the section shown.
    fn leave_reader(&mut self, cx: &mut Context<Self>) {
        leave(&mut self.documents, &mut self.reports);
        self.refetch(true, cx);
    }

    /// Current plans read Backbone only while their section is selected.
    fn plans_shown(&self, shown: bool, cx: &mut Context<Self>) {
        if let Some(plans) = &self.plans {
            plans.update(cx, |plans, cx| plans.set_shown(shown, cx));
        }
    }

    fn show_plans(&mut self, cx: &mut Context<Self>) {
        self.interactions_shown(false, cx);
        let Some(connection) = &self.connection else {
            return;
        };
        let source = plans::Source(connection.clone());
        if !source.belongs_to(&self.agents) {
            return;
        }
        self.leave_reader(cx);
        match &self.plans {
            Some(_) => self.plans_shown(true, cx),
            None => {
                let visible = self.visible.clone();
                let plans =
                    cx.new(|cx| plans::Plans::new(source, visible, self.agents.clone(), cx));
                self._search
                    .push(cx.observe(&plans, |_, _, cx| cx.notify()));
                self.plans = Some(plans);
            }
        }
        self.shown = Shown::Plans;
        cx.notify();
    }

    fn show_sheets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        let source = sheets::Source(connection.clone());
        if !source.belongs_to(&self.agents) {
            return;
        }
        self.interactions_shown(false, cx);
        self.leave_reader(cx);
        self.plans_shown(false, cx);
        if self.sheets.is_none() {
            let sheets = cx.new(|cx| sheets::Sheets::new(source, window, cx));
            // An opened workbook trades the section bar for its own.
            self._search
                .push(cx.observe(&sheets, |_, _, cx| cx.notify()));
            self.sheets = Some(sheets);
        }
        self.shown = Shown::Sheets;
        cx.notify();
    }

    fn interactions_shown(&self, shown: bool, cx: &mut Context<Self>) {
        if let Some(view) = &self.interactions {
            view.update(cx, |view, cx| view.set_shown(shown, cx));
        }
    }

    fn show_interactions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        let source = interactions::Source(connection.clone());
        if !source.belongs_to(&self.agents) {
            return;
        }
        self.leave_reader(cx);
        self.plans_shown(false, cx);
        if self.interactions.is_none() {
            let visible = self.visible.clone();
            // A linked report is read through the same connection as the history.
            let reports = source.clone();
            let view = cx.new(|cx| {
                interactions::Interactions::new(source, visible, self.agents.clone(), window, cx)
            });
            self._search.push(cx.observe(&view, |_, _, cx| cx.notify()));
            self._search.push(cx.subscribe(
                &view,
                move |_, _, event: &interactions::Report, cx| {
                    let interactions::Report {
                        id,
                        response,
                        request,
                        anchor,
                    } = event;
                    let source = reports.clone();
                    let id = *id;
                    let response = *response;
                    let request = *request;
                    let anchor = anchor.clone();
                    let worker = cx
                        .background_executor()
                        .spawn(async move { source.report(id, response) });
                    cx.spawn(async move |this, cx| {
                        let result = worker.await;
                        let _ = this.update(cx, |pane, cx| {
                            if pane.shown != Shown::Interactions
                                || !pane
                                    .interactions
                                    .as_ref()
                                    .is_some_and(|view| view.read(cx).is_selected(&anchor, request))
                            {
                                return;
                            }
                            let link_error = result.as_ref().err().copied();
                            if let Ok(page) = result
                                && let Some(item) = page.items.first()
                            {
                                pane.show_records(
                                    if response {
                                        views::ReportQuery::Responses(None)
                                    } else {
                                        views::ReportQuery::Recent(None)
                                    },
                                    cx,
                                );
                                pane.reports.open = Some(item.clone());
                                pane.data = Some(Data::Reports(page));
                                pane.read_at = Some(std::time::Instant::now());
                                // Follow the opened record so its updates are kept.
                                pane.refetch(false, cx);
                            } else if let Some(error) = link_error
                                && let Some(view) = &pane.interactions
                            {
                                view.update(cx, |view, cx| view.link_failed(error, cx));
                            }
                        });
                    })
                    .detach();
                },
            ));
            self.interactions = Some(view);
        } else {
            self.interactions_shown(true, cx);
        }
        self.shown = Shown::Interactions;
        cx.notify();
    }

    /// Ask the worker for the current request now. `clear` drops data that
    /// would be wrong under the new request, such as another range's totals.
    fn refetch(&mut self, clear: bool, cx: &mut Context<Self>) {
        if clear {
            self.data = None;
            // Without a worker nothing will arrive, so keep saying why.
            if self.source.is_some() {
                self.error = None;
            }
        }
        if let Some(source) = &self.source {
            source.set_request(self.request());
        }
        cx.notify();
    }

    fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if self.range != range {
            self.range = range;
            // Only a Usage pane has these controls. The old range's totals
            // must not appear under the new label.
            self.refetch(true, cx);
        }
    }

    /// The documents the shown list can open, in its order.
    fn targets(&self) -> Vec<Target> {
        match &self.data {
            Some(Data::Skills(library)) => skills::targets(self, library),
            Some(Data::Policies(library)) => policies::targets(library),
            Some(Data::Agent(agent)) => agent::targets(self, agent),
            _ => Vec::new(),
        }
    }

    /// Open `target` in the reader. A skill's SKILL.md and a catalog policy's
    /// text are read from their snapshots off the UI thread; any other policy
    /// shows the text Backbone listed with it.
    fn open(&mut self, target: Target, cx: &mut Context<Self>) {
        if let Target::Policy(path) = &target
            && let Some(Data::Policies(library)) = &self.data
            && let Some(source) = library
                .all()
                .into_iter()
                .find(|source| source.path.as_ref() == path)
        {
            self.open_policy(source.clone(), library.members.clone().into(), cx);
            return;
        }
        if let Target::Policy(path) = &target
            && let Some(Data::Agent(agent)) = &self.data
            && let Some(Ok(brief)) = &agent.brief
            && let Some(source) = brief
                .sources
                .iter()
                .find(|source| source.path.as_ref() == path)
        {
            self.open_policy(source.clone(), [agent.name.clone()].into(), cx);
            return;
        }
        self.open_with_skill(target, None, cx);
    }

    fn open_policy(
        &mut self,
        source: views::Source,
        candidates: std::sync::Arc<[String]>,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = &self.connection else {
            return;
        };
        let bound = bound_policies::Open::capture(connection, source, candidates);
        let initial = bound.source.text.as_ref().map(|text| Ok(text.clone()));
        let artifact = bound.artifact.clone();
        let ticket = bound.ticket();
        self.open = Some(Open {
            target: Target::Policy(bound.source.path.to_string()),
            bound_skill: None,
            bound_policy: Some(bound),
            text: initial,
        });
        if artifact.is_none() {
            if let Some(open) = &mut self.open {
                open.text
                    .get_or_insert(Err("This policy has no provider snapshot available."));
            }
            cx.notify();
            return;
        }
        let read_ticket = ticket.clone();
        let read = cx
            .background_executor()
            .spawn(async move { bound_policies::read(artifact, &read_ticket) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                if let Some(open) = &mut pane.open
                    && let Some(bound) = &mut open.bound_policy
                    && bound.accepts(&ticket)
                {
                    open.text = Some(match result {
                        Ok(snapshot) => {
                            let text = snapshot
                                .content
                                .clone()
                                .ok_or("This policy's source is unavailable.");
                            bound.snapshot = Some(snapshot);
                            text.map(Into::into)
                        }
                        Err(error) => Err(error),
                    });
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn open_with_skill(
        &mut self,
        target: Target,
        captured: Option<(Option<views::Skill>, std::sync::Arc<[String]>)>,
        cx: &mut Context<Self>,
    ) {
        let bound_skill = self.connection.as_ref().and_then(|connection| {
            let Target::Skill(path) = &target else {
                return None;
            };
            let (record, team) = match &captured {
                Some((record, team)) => (record.as_ref(), team.clone()),
                None => match &self.data {
                    Some(Data::Skills(library)) => (
                        library.store.items.iter().find(|skill| &skill.path == path),
                        library
                            .agents
                            .iter()
                            .map(|agent| agent.name.clone())
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                    // An agent's skill is offered to that agent.
                    Some(Data::Agent(agent)) => (
                        agent
                            .skills
                            .as_ref()
                            .and_then(|loadout| loadout.as_ref().ok())
                            .and_then(|loadout| {
                                loadout.skills.iter().find(|skill| &skill.path == path)
                            }),
                        [agent.name.clone()].into(),
                    ),
                    _ => (None, std::sync::Arc::from([])),
                },
            };
            Some(bound_skills::Open::capture(connection, record, &team))
        });
        self.open = Some(Open {
            target,
            bound_skill,
            bound_policy: None,
            text: None,
        });
        // Only a profile's skill has a source to read.
        let Some(bound) = self
            .open
            .as_ref()
            .and_then(|open| open.bound_skill.as_ref())
        else {
            cx.notify();
            return;
        };
        let ticket = bound.ticket();
        let artifact = bound.artifact.clone();
        let read_ticket = ticket.clone();
        let read = cx
            .background_executor()
            .spawn(async move { bound_skills::read(artifact, &read_ticket) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                if let Some(open) = &mut pane.open
                    && let Some(bound) = &mut open.bound_skill
                    && bound.accepts(&ticket)
                {
                    open.text = Some(match result {
                        Ok(snapshot) => {
                            let text = snapshot
                                .content
                                .clone()
                                .ok_or("This skill's source is unavailable.");
                            bound.snapshot = Some(snapshot);
                            text.map(Into::into)
                        }
                        Err(error) => Err(error),
                    });
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Back from the reader, or from a tag's agents, to the list.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.carriers_of = None;
        cx.notify();
    }

    /// Open the previous or next document of the list that opened this one.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(open) = &self.open else {
            return;
        };
        let targets = self.targets();
        let Some(at) = targets.iter().position(|target| *target == open.target) else {
            return;
        };
        if let Some(target) = at.checked_add_signed(by).and_then(|next| targets.get(next)) {
            self.open(target.clone(), cx);
        }
    }

    /// Open the newer (`by < 0`) or older record beside the open report or answer.
    fn step_record(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(Data::Reports(page)) = self.data.clone() {
            reports::step(self, &page, by, cx);
        }
    }

    /// Keys a view understands while its pane is selected. Returns whether
    /// `key` was one of them.
    pub fn key(
        &mut self,
        key: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(pane) = &self.connections
            && key.key == "tab"
            && !key.modifiers.modified()
        {
            pane.update(cx, |pane, cx| pane.focus_entry(window, cx));
            return true;
        }
        if let Some(view) = self
            .interactions
            .clone()
            .filter(|_| self.shown == Shown::Interactions)
        {
            return view.update(cx, |view, cx| view.key(key, window, cx));
        }
        if self.selection.borrow_mut().key(key, cx) {
            return true;
        }
        if key.modifiers.modified() {
            return false;
        }
        if let Some(plans) = self.plans.clone().filter(|_| self.shown == Shown::Plans) {
            let agent = self.reports.agent.clone();
            return plans::key(&plans, agent.as_deref(), key, cx);
        }
        if let Some(conversation) = self.conversation.clone().filter(|_| key.key == "c") {
            conversation.update(cx, |conversation, cx| conversation.request_toggle(cx));
            return true;
        }
        if self.reports.open.is_some() && documents::selected(self).is_none() {
            match key.key.as_str() {
                "[" => {
                    return {
                        self.step_record(-1, cx);
                        true
                    };
                }
                "]" => {
                    return {
                        self.step_record(1, cx);
                        true
                    };
                }
                _ => {}
            }
        }
        if self.reports.open.is_some() && key.key == "escape" {
            self.documents.clear();
            self.reports.open = None;
            self.refetch(true, cx);
            return true;
        }
        if self.open.is_none() {
            if self.carriers_of.is_some() && key.key == "escape" {
                self.close(cx);
                return true;
            }
            return false;
        }
        match key.key.as_str() {
            "escape" => self.close(cx),
            "[" => self.step(-1, cx),
            "]" => self.step(1, cx),
            _ => return false,
        }
        true
    }

    /// The title shown for a view.
    pub fn title(view: &View) -> SharedString {
        match &view.agent {
            Some(agent) => agent.clone().into(),
            None => about(view.kind).label.into(),
        }
    }

    pub(super) fn run_team_action(
        &mut self,
        name: String,
        registration: Option<String>,
        action: TeamAgentAction,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        if !self.team_actions_in_flight.insert(name.clone()) {
            return;
        }
        self.team_notice = Some(match action {
            TeamAgentAction::Start { resume: true } => format!("Resuming {name}…"),
            TeamAgentAction::Start { resume: false } => format!("Starting {name}…"),
            TeamAgentAction::Stop => format!("Stopping {name}…"),
            TeamAgentAction::Restart { .. } => format!("Restarting {name}…"),
        });
        let agent = name.clone();
        let work = cx.background_executor().spawn(async move {
            match action {
                TeamAgentAction::Start { resume } => {
                    connection.start_agent(&agent, resume).roster_message()
                }
                TeamAgentAction::Stop => connection
                    .stop_agent(&agent, registration.as_deref())
                    .message(),
                TeamAgentAction::Restart { resume } => {
                    connection.restart_agent(&agent, resume).message()
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let message = work.await;
            let _ = this.update(cx, |pane, cx| {
                pane.team_actions_in_flight.remove(&name);
                pane.team_notice = Some(message);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Drop for ViewPane {
    // Stopping the worker waits for its current request, which can take
    // seconds; never do that on the UI thread.
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            std::thread::spawn(move || drop(source));
        }
    }
}

impl Render for ViewPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(card) = self.bound_agent.clone().filter(|_| self.open.is_none()) {
            // Back from the reader: its conversation stops reading, kept with
            // its draft, and its text can no longer be selected or copied.
            if let Some(conversation) = self.conversation.take() {
                conversation.update(cx, |conversation, cx| conversation.set_active(false, cx));
            }
            let mut selection = self.selection.borrow_mut();
            selection.begin();
            selection.finish();
            return div().size_full().child(card);
        }
        if let Some(connections) = &self.connections {
            return div().size_full().child(connections.clone());
        }
        if let Some(artifacts) = &self.artifacts {
            return div().size_full().child(artifacts.clone());
        }
        if let Some(view) = self
            .interactions
            .clone()
            .filter(|_| self.shown == Shown::Interactions)
        {
            if let Some(conversation) = self.conversation.take() {
                conversation.update(cx, |c, cx| c.set_active(false, cx));
            }
            let frame = if view.read(cx).is_open() {
                reports::plain_frame(None, view)
            } else {
                reports::section_frame(self, view, cx)
            };
            return div().size_full().bg(colors().surface).child(frame);
        }
        let sheets = self.sheets.clone().filter(|_| self.shown == Shown::Sheets);
        let plans = self.plans.clone().filter(|_| self.shown == Shown::Plans);
        if sheets.is_some()
            && let Some(conversation) = self.conversation.take()
        {
            conversation.update(cx, |conversation, cx| conversation.set_active(false, cx));
        }
        if let Some(sheets) = sheets {
            let frame = if sheets.read(cx).is_open() {
                reports::plain_frame(None, sheets)
            } else {
                reports::section_frame(self, sheets, cx)
            };
            return div().size_full().bg(colors().surface).child(frame);
        }
        if let Some(plans) = plans {
            let previous = self.conversation.take();
            self.conversation = plans.read(cx).conversation().map(|target| {
                let key = target.key();
                self.conversations
                    .entry(key)
                    .or_insert_with(|| {
                        let conversation = cx.new(|cx| {
                            conversation::Conversation::about_plan(
                                &target,
                                self.visible.clone(),
                                window,
                                cx,
                            )
                        });
                        self._search.push(cx.observe(&conversation, |pane, _, cx| {
                            let held = pane
                                .conversations
                                .values()
                                .any(|conversation| conversation.read(cx).holds_plan(cx));
                            if let Some(plans) = &pane.plans {
                                plans.update(cx, |plans, cx| plans.hold_refresh(held, cx));
                            }
                            cx.notify();
                        }));
                        conversation
                    })
                    .clone()
            });
            if previous != self.conversation {
                for (conversation, active) in [(previous, false), (self.conversation.clone(), true)]
                {
                    if let Some(conversation) = conversation {
                        conversation
                            .update(cx, |conversation, cx| conversation.set_active(active, cx));
                    }
                }
            }
            // Plans render through the pane so their text can be selected.
            let body = loop {
                self.selection.borrow_mut().begin();
                let body = plans::body(self, &plans, window, cx);
                if !self.selection.borrow_mut().finish() {
                    break body;
                }
            };
            return div()
                .relative()
                .size_full()
                .bg(colors().surface)
                .child(body)
                .child(selection::events(cx));
        }
        let (t, c) = (tokens(), colors());
        let previous = self.conversation.take();
        self.conversation = self
            .reports
            .open
            .as_ref()
            .zip(self.connection.as_ref())
            .map(|(item, connection)| {
                let key = format!(
                    "{}\n{}\n{}",
                    seen::origin_key(connection.identity()),
                    item.author_id.as_deref().unwrap_or_default(),
                    agent_interface_roster::feedback::reference(item)
                );
                self.conversations
                    .entry(key)
                    .or_insert_with(|| {
                        cx.new(|cx| {
                            conversation::Conversation::new(
                                self.agents.clone(),
                                item.clone(),
                                connection.clone(),
                                self.documents.surface.clone(),
                                self.visible.clone(),
                                window,
                                cx,
                            )
                        })
                    })
                    .clone()
            });
        // Bound skill discussions retain the exact displayed artifact revision.
        if self.conversation.is_none()
            && let Some(bound) = self
                .open
                .as_ref()
                .and_then(|open| open.bound_skill.as_ref())
            && let Some(snapshot) = &bound.snapshot
        {
            let key = bound_skills::key(snapshot);
            let candidates = bound.candidates.clone();
            self.conversation = Some(
                self.conversations
                    .entry(key)
                    .or_insert_with(|| {
                        cx.new(|cx| {
                            conversation::Conversation::about_snapshot(
                                snapshot,
                                candidates,
                                self.visible.clone(),
                                window,
                                cx,
                            )
                        })
                    })
                    .clone(),
            );
        }
        if self.conversation.is_none()
            && let Some(bound) = self
                .open
                .as_ref()
                .and_then(|open| open.bound_policy.as_ref())
            && let Some(snapshot) = &bound.snapshot
        {
            let key = bound_policies::key(snapshot);
            self.conversation = Some(
                self.conversations
                    .entry(key)
                    .or_insert_with(|| {
                        cx.new(|cx| {
                            conversation::Conversation::about_snapshot(
                                snapshot,
                                bound.candidates.to_vec(),
                                self.visible.clone(),
                                window,
                                cx,
                            )
                        })
                    })
                    .clone(),
            );
        }
        // Only the shown conversation reads Backbone.
        if previous != self.conversation {
            for (conversation, active) in [(previous, false), (self.conversation.clone(), true)] {
                if let Some(conversation) = conversation {
                    conversation.update(cx, |conversation, cx| conversation.set_active(active, cx));
                }
            }
        }
        let about = about(self.view.kind);
        // Lent to the body for this frame and put back below.
        let data = self.data.take();
        let body = loop {
            self.selection.borrow_mut().begin();
            let body = if documents::selected(self).is_some() {
                documents::reader(self, cx)
            } else {
                match &data {
                    None => {
                        let state = match self.error {
                            Some(error) => ui::empty_state(
                                ui::empty_tile("unplug", c.selected, c.muted),
                                if self.view.kind == ViewKind::Connections {
                                    "Connections unavailable"
                                } else {
                                    "No data"
                                },
                                error,
                            ),
                            None => ui::empty_state(
                                ui::empty_tile(about.icon, c.selected, c.muted),
                                "Reading Agent Backbone…",
                                "",
                            ),
                        };
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(ui::view_header(about.icon, ViewPane::title(&self.view)))
                            // Without a worker, as with no profile, the controls
                            // would fetch nothing.
                            .when(
                                self.source.is_some() && self.view.kind == ViewKind::Usage,
                                |el| el.child(usage::toolbar(self, cx)),
                            )
                            .when(
                                self.source.is_some() && self.view.kind == ViewKind::Reports,
                                |el| el.child(reports::refresh_toolbar(self, cx)),
                            )
                            .child(state)
                            .into_any_element()
                    }
                    Some(Data::Team(data)) => team::body(self, data, cx),
                    Some(Data::Usage(data)) => usage::body(self, data, cx),
                    Some(Data::Activity(data)) => activity::body(self, data, cx),
                    Some(Data::Skills(data)) => skills::body(self, data, cx),
                    Some(Data::Agent(data)) => agent::body(self, data, cx),
                    Some(Data::Policies(data)) => policies::body(self, data, cx),
                    Some(Data::Tags(data)) => tags::body(self, data, cx),
                    Some(Data::Reports(data)) => reports::body(self, data, cx),
                    Some(Data::Document(_)) => unreachable!("document data has a selected reader"),
                }
            };
            if !self.selection.borrow_mut().finish() {
                break body;
            }
            // A later block changed after earlier highlights were constructed.
            // Rebuild with the cleared selection before this body can be cached.
        };
        self.data = data;
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(selection::events(cx))
            .children(
                self.data
                    .is_some()
                    .then_some(self.error)
                    .flatten()
                    .map(|error| {
                        ui::text(
                            div()
                                .flex_shrink_0()
                                .px(px(t.space.s14))
                                .py(px(t.space.s4))
                                .bg(c.attention_soft)
                                .text_color(c.attention_text),
                            &t.text.meta,
                        )
                        .child(stale_banner(error, self.read_at))
                    }),
            )
    }
}

/// A scrolling list area under a view's toolbar.
fn list(id: &'static str) -> gpui::Stateful<gpui::Div> {
    let t = tokens();
    div()
        .id(id)
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .px(px(t.space.s8))
        .py(px(t.space.s6))
}

/// A token count in a few characters: 4.82M, 312k, 950.
fn count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{}k", tokens / 1_000),
        1_000_000..1_000_000_000 => format!("{:.2}M", tokens as f64 / 1e6),
        _ => format!("{:.2}B", tokens as f64 / 1e9),
    }
}

/// A view's read that could not be made, unlike a refusal, which keeps
/// [`views::UNAVAILABLE`].
const NOT_ANSWERING: &str = "Backbone is not answering.";

/// What a view says over the last data it read when a newer read failed.
fn stale_banner(error: &str, read_at: Option<std::time::Instant>) -> String {
    if error == NOT_ANSWERING {
        super::agent_status::not_answering(read_at)
    } else {
        format!("{error} Showing the last data read.")
    }
}

/// How long ago, in one unit: 40s, 6m, 2h, 3d.
pub(super) fn ago(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// How long ago an epoch time was.
fn since(at: Option<f64>) -> String {
    at.map(|at| ago(views::now() as f64 - at))
        .unwrap_or_default()
}

/// A path with the home directory written as `~`.
fn home(path: &str) -> String {
    super::pane_cards::home_relative(path, std::env::var("HOME").ok().as_deref())
}

/// Text without control characters, on one line.
fn line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_whose_read_could_not_be_made_says_how_old_its_data_is() {
        let read = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(400));
        assert_eq!(
            stale_banner(NOT_ANSWERING, read),
            "Backbone is not answering · last read 6m ago"
        );
        assert_eq!(
            stale_banner(views::UNAVAILABLE, read),
            "Backbone could not provide this view. Showing the last data read."
        );
    }

    #[test]
    fn leaving_the_reader_closes_its_record_so_a_late_read_is_refused() {
        use agent_interface_roster::views::{
            DocumentMetadata, DocumentRevision, ReportContent, ReportItem,
        };
        let metadata = DocumentMetadata {
            id: 7,
            document_id: "a".repeat(32),
            author_id: "b".repeat(32),
            author_name: "worker".into(),
            agent_name: None,
            title: "Title".into(),
            created_at: "2026-11-19T00:00:00Z".into(),
            content_bytes: 1,
            content_sha256: "c".repeat(64),
            supersedes_id: None,
            superseded_by: None,
        };
        let mut reports = reports::State::default();
        reports.open = Some(ReportItem {
            id: 7,
            author: "worker".into(),
            author_id: Some("b".repeat(32)),
            registered: false,
            created_at: metadata.created_at.clone(),
            conversation_id: None,
            content: ReportContent::Document(metadata.clone()),
        });
        let mut documents = documents::State::default();
        documents.loaded = Some(DocumentRevision {
            metadata,
            html: "x".into(),
        });
        assert_eq!(reports_request(&reports), Request::Document(7));
        leave(&mut documents, &mut reports);
        assert!(reports.open.is_none() && documents.loaded.is_none());
        // A read of the Document still in flight no longer matches.
        assert_ne!(reports_request(&reports), Request::Document(7));
    }

    #[test]
    fn counts_and_ages_stay_short() {
        assert_eq!(count(950), "950");
        assert_eq!(count(312_400), "312k");
        assert_eq!(count(4_820_000), "4.82M");
        assert_eq!(count(40_180_741_701), "40.18B");
        assert_eq!(ago(40.0), "40s");
        assert_eq!(ago(400.0), "6m");
        assert_eq!(ago(7_300.0), "2h");
        assert_eq!(ago(-5.0), "0s");
        assert_eq!(line("a\nb"), "a b");
    }

    #[test]
    fn every_kind_has_a_distinct_label() {
        let labels: HashSet<_> = ViewKind::ALL
            .iter()
            .map(|kind| about(*kind).label)
            .collect();
        assert_eq!(labels.len(), ViewKind::ALL.len());
    }
}
