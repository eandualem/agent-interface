//! Workspace, tab and split models, independent of storage and rendering.

#![forbid(unsafe_code)]

mod actions;
mod geometry;
pub mod keymap;
pub mod merge;
mod navigation;

pub use actions::{AttachmentTarget, NameEdit, RenameOutcome};
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
}

/// A session name on its original tmux server, even while offline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// Raw session name, not a tmux command target.
    pub session: String,
    /// Explicit absolute socket path, or the default source server.
    pub source_socket: Option<String>,
}
