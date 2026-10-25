//! Workspace, tab and split models, independent of storage and rendering.

#![forbid(unsafe_code)]

mod actions;
mod choices;
mod geometry;
pub mod keymap;
pub mod merge;
mod navigation;
pub mod theme;

pub use actions::{AttachmentTarget, NameEdit, RenameOutcome, WORKSPACE_ICONS};
pub use choices::SessionChoices;
pub use geometry::PaneRect;

/// An arrangement loaded from a library. Navigation values seed a new window;
/// each window subsequently owns its selection and focus.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Workspaces in saved order.
    pub workspaces: Vec<Workspace>,
    /// Initially selected workspace identity.
    pub selected: String,
    /// Show only the selected pane; this is separate from keyboard input focus.
    pub focus: bool,
    /// Whether the optional roster is shown.
    pub show_agents: bool,
}

/// A named group of tabs; it may be empty.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    /// Stable 12-character hexadecimal identity.
    pub id: String,
    /// User-assigned name.
    pub name: String,
    /// Optional one- or two-character icon.
    pub icon: Option<String>,
    /// Optional named native icon, which takes precedence over `icon`.
    pub symbol: Option<String>,
    /// Tabs in saved order.
    pub tabs: Vec<Tab>,
    /// Initially selected tab, or the empty string for an empty workspace.
    pub selected: String,
}

/// A named split tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// Stable identity.
    pub id: String,
    /// User-assigned name, independent of attached session names.
    pub name: String,
    /// Split arrangement.
    pub tree: Node,
    /// Initially focused leaf identity.
    pub focus: String,
}

/// One terminal area or a split between two areas.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// An ordinary shell, attachment or empty chooser.
    Leaf(Leaf),
    /// A split with a stable identity of its own.
    Split {
        /// Stable identity.
        id: String,
        /// Position of the second child relative to the first.
        direction: Direction,
        /// Fraction assigned to the first child, from 0.01 through 0.99.
        ratio: f64,
        /// First child in navigation order.
        first: Box<Node>,
        /// Second child in navigation order.
        second: Box<Node>,
    },
}

/// Split orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Second child to the right.
    Right,
    /// Second child below.
    Below,
}

/// A stable terminal area. An attachment does not replace its ordinary shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    /// Stable identity, also used by the ordinary shell session.
    pub id: String,
    /// An existing session, or no attachment for an ordinary shell/chooser.
    pub attachment: Option<Attachment>,
    /// Saved absolute working directory, when present.
    pub cwd: Option<String>,
    /// Show the new-pane chooser instead of an ordinary shell.
    pub empty: bool,
    /// Show an Agent Backbone view instead of a terminal.
    pub view: Option<View>,
}

impl Leaf {
    /// Whether this area shows a terminal: neither the new-pane chooser nor a view.
    pub fn runs_terminal(&self) -> bool {
        !self.empty && self.view.is_none()
    }
}

/// Information shown in a pane in place of a terminal. It starts no process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// What the pane shows.
    pub kind: ViewKind,
    /// The agent an agent view describes; none for every other kind.
    pub agent: Option<String>,
    /// The section an agent view opens on; none for every other kind.
    pub section: Option<AgentSection>,
}

impl View {
    /// A view that describes no agent.
    pub fn of(kind: ViewKind) -> Self {
        Self {
            kind,
            agent: None,
            section: None,
        }
    }

    /// One agent's view, opening on `section`.
    pub fn agent(name: String, section: Option<AgentSection>) -> Self {
        Self {
            kind: ViewKind::Agent,
            agent: Some(name),
            section,
        }
    }

    /// Whether the agent and section fit the kind: only an agent view names an
    /// agent, and it alone may open on a section.
    pub fn is_valid(&self) -> bool {
        let agent = self.kind == ViewKind::Agent;
        agent == self.agent.is_some() && (agent || self.section.is_none())
    }
}

/// The views a pane can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    /// Agents by repository, with their states.
    Team,
    /// Token usage by agent.
    Usage,
    /// Recent deliveries and inbound events.
    Activity,
    /// Every skill an agent can load, by where it lives, team-wide.
    Skills,
    /// One agent: its state, rules, connections, skills, policies and usage.
    Agent,
    /// The layers a brief is built from, team-wide.
    Policies,
    /// What each tag gives the agents that carry it.
    Tags,
    /// Authored progress updates and retained answers.
    Reports,
    /// Locally published HTML documents.
    Artifacts,
}

impl ViewKind {
    /// Every kind, in the order the new-pane chooser lists them.
    pub const ALL: [Self; 9] = [
        Self::Skills,
        Self::Policies,
        Self::Tags,
        Self::Team,
        Self::Usage,
        Self::Activity,
        Self::Agent,
        Self::Reports,
        Self::Artifacts,
    ];

    /// The saved name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Team => "team",
            Self::Usage => "usage",
            Self::Activity => "activity",
            Self::Skills => "skills",
            Self::Agent => "agent",
            Self::Policies => "policies",
            Self::Tags => "tags",
            Self::Reports => "reports",
            Self::Artifacts => "artifacts",
        }
    }

    /// Whether this view describes one agent, which it then names.
    pub fn needs_agent(self) -> bool {
        self == Self::Agent
    }

    /// The kind with this saved name.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

/// The sections of one agent's view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentSection {
    /// Its state, work and held messages.
    #[default]
    Overview,
    /// What its CLI lets it do without asking.
    Permissions,
    /// GitHub, sources and other connections.
    Connections,
    /// The skills it loads, and why each reaches it.
    Skills,
    /// What it receives at its next start, in order.
    Policies,
    /// Its token usage.
    Usage,
}

impl AgentSection {
    /// Every section, in tab order.
    pub const ALL: [Self; 6] = [
        Self::Overview,
        Self::Permissions,
        Self::Connections,
        Self::Skills,
        Self::Policies,
        Self::Usage,
    ];

    /// The saved name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Permissions => "permissions",
            Self::Connections => "connections",
            Self::Skills => "skills",
            Self::Policies => "policies",
            Self::Usage => "usage",
        }
    }

    /// The section with this saved name.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|section| section.as_str() == name)
    }
}

/// A session name on its original tmux server, even while offline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// Raw session name, not a tmux command target.
    pub session: String,
    /// Explicit absolute socket path, or the default source server.
    pub source_socket: Option<String>,
}
