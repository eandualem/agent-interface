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
    /// The shared skills store.
    Skills,
    /// One agent's card.
    Agent,
    /// The policies one agent receives, in order.
    Policies,
}

impl ViewKind {
    /// Every kind, in the order the new-pane chooser lists them.
    pub const ALL: [Self; 6] = [
        Self::Skills,
        Self::Policies,
        Self::Agent,
        Self::Usage,
        Self::Activity,
        Self::Team,
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
        }
    }

    /// Whether this view describes one agent, which it then names.
    pub fn needs_agent(self) -> bool {
        matches!(self, Self::Agent | Self::Policies)
    }

    /// The kind with this saved name.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
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
