//! In-memory equivalents of the Python workspace model operations. These do
//! not persist records, validate proposed user input, or start/stop processes.

use crate::{Attachment, Direction, Layout, Leaf, Node, Tab, Workspace};

fn identity() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

fn leaf(cwd: String, empty: bool) -> Leaf {
    Leaf {
        id: identity(),
        attachment: None,
        cwd: Some(cwd),
        empty,
    }
}

fn tab(name: Option<String>, cwd: String, empty: bool) -> Tab {
    let pane = leaf(cwd, empty);
    Tab {
        id: identity(),
        name: name
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Tab {}", &identity()[..4])),
        focus: pane.id.clone(),
        tree: Node::Leaf(pane),
    }
}

impl Layout {
    /// A new arrangement with Workspace 1, a randomly named tab and one ordinary
    /// shell reference. `cwd` must be an absolute, NUL-free working directory.
    /// This creates records only; it does not initialize a database or shell.
    pub fn initial(cwd: String) -> Self {
        let tab = tab(None, cwd, false);
        let space = Workspace {
            id: identity(),
            name: "Workspace 1".into(),
            icon: None,
            selected: tab.id.clone(),
            tabs: vec![tab],
        };
        Self {
            selected: space.id.clone(),
            workspaces: vec![space],
            focus: false,
            show_agents: false,
        }
    }

    /// The selected workspace, falling back to the first available one.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|space| space.id == self.selected)
            .or_else(|| self.workspaces.first())
    }

    fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        let index = self
            .workspaces
            .iter()
            .position(|space| space.id == self.selected)
            .unwrap_or(0);
        self.workspaces.get_mut(index)
    }

    /// The selected tab, or no tab when the selected workspace is empty.
    pub fn tab(&self) -> Option<&Tab> {
        let space = self.workspace()?;
        space
            .tabs
            .iter()
            .find(|tab| tab.id == space.selected)
            .or_else(|| space.tabs.first())
    }

    fn tab_mut(&mut self) -> Option<&mut Tab> {
        let space = self.workspace_mut()?;
        let index = space
            .tabs
            .iter()
            .position(|tab| tab.id == space.selected)
            .unwrap_or(0);
        space.tabs.get_mut(index)
    }

    /// The focused terminal area in the selected tab.
    pub fn pane(&self) -> Option<&Leaf> {
        let tab = self.tab()?;
        tab.tree
            .leaves()
            .into_iter()
            .find(|leaf| leaf.id == tab.focus)
    }

    fn pane_mut(&mut self) -> Option<&mut Leaf> {
        let tab = self.tab_mut()?;
        match find_leaf(&mut tab.tree, &tab.focus)? {
            Node::Leaf(leaf) => Some(leaf),
            Node::Split { .. } => unreachable!("find_leaf only returns leaves"),
        }
    }

    /// Select an existing workspace by identity. Missing identities do nothing.
    pub fn choose_workspace(&mut self, id: &str) -> bool {
        if !self.workspaces.iter().any(|space| space.id == id) {
            return false;
        }
        self.selected = id.to_owned();
        true
    }

    /// Select an existing tab in the current workspace, retaining its pane focus.
    pub fn choose_tab(&mut self, id: &str) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        if !space.tabs.iter().any(|tab| tab.id == id) {
            return false;
        }
        space.selected = id.to_owned();
        true
    }

    /// Cycle workspaces, wrapping at either end and retaining each selection.
    pub fn next_workspace(&mut self, offset: isize) -> bool {
        let count = self.workspaces.len() as isize;
        if count == 0 {
            return false;
        }
        let index = self
            .workspaces
            .iter()
            .position(|space| space.id == self.selected)
            .unwrap_or(0);
        let next = (index as isize + offset.rem_euclid(count)).rem_euclid(count) as usize;
        self.selected = self.workspaces[next].id.clone();
        true
    }

    /// Cycle tabs in the current workspace, wrapping at either end.
    pub fn next_tab(&mut self, offset: isize) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        let count = space.tabs.len() as isize;
        if count == 0 {
            return false;
        }
        let index = space
            .tabs
            .iter()
            .position(|tab| tab.id == space.selected)
            .unwrap_or(0);
        let next = (index as isize + offset.rem_euclid(count)).rem_euclid(count) as usize;
        space.selected = space.tabs[next].id.clone();
        true
    }

    /// Move the selected tab within its workspace, stopping at either end.
    pub fn move_tab(&mut self, offset: isize) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        if space.tabs.is_empty() {
            return false;
        }
        let index = space
            .tabs
            .iter()
            .position(|tab| tab.id == space.selected)
            .unwrap_or(0);
        let target = index
            .saturating_add_signed(offset)
            .min(space.tabs.len() - 1);
        let tab = space.tabs.remove(index);
        space.tabs.insert(target, tab);
        true
    }

    /// Move the selected tab to another workspace and select it there. Retain
    /// its complete tree and focus; the source selects its first remaining tab.
    /// Missing or current destinations, and empty sources, do nothing.
    pub fn transfer_tab(&mut self, destination: &str) -> bool {
        let Some(source) = self.workspace() else {
            return false;
        };
        if source.id == destination || !self.workspaces.iter().any(|space| space.id == destination)
        {
            return false;
        }
        let Some(tab) = self.tab().cloned() else {
            return false;
        };
        self.close_tab();
        self.selected = destination.to_owned();
        let space = self.workspace_mut().expect("destination exists");
        space.selected = tab.id.clone();
        space.tabs.push(tab);
        true
    }

    /// Delete the current workspace only when empty and another workspace exists.
    /// Select the first remaining workspace without changing any tab or process.
    pub fn delete_workspace(&mut self) -> bool {
        let Some(space) = self.workspace() else {
            return false;
        };
        if !space.tabs.is_empty() || self.workspaces.len() <= 1 {
            return false;
        }
        let id = space.id.clone();
        self.workspaces.retain(|space| space.id != id);
        self.selected = self.workspaces[0].id.clone();
        true
    }

    /// Add and select an empty workspace. The caller validates the proposed name;
    /// adding a workspace never populates tabs from sessions or the roster.
    pub fn add_workspace(&mut self, name: String) {
        let space = Workspace {
            id: identity(),
            name,
            icon: None,
            tabs: vec![],
            selected: String::new(),
        };
        self.selected = space.id.clone();
        self.workspaces.push(space);
    }

    /// Add and select one tab in the current workspace. `empty` selects a new-pane
    /// chooser rather than an ordinary shell reference. Names and cwd are supplied
    /// by the caller; persisted-schema validation remains required before saving.
    /// Returns false only when there is no workspace.
    pub fn add_tab(&mut self, name: Option<String>, cwd: String, empty: bool) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        let tab = tab(name, cwd, empty);
        space.selected = tab.id.clone();
        space.tabs.push(tab);
        true
    }

    /// Split the focused leaf, retaining its identity and attachment as the first
    /// child and focusing the new second child. An empty workspace gets a tab.
    /// The caller supplies cwd and checks schema limits before persisting.
    /// Returns false if the arrangement has no workspace or focused leaf.
    pub fn split(&mut self, direction: Direction, cwd: String, empty: bool) -> bool {
        let Some(tab) = self.tab_mut() else {
            return self.add_tab(None, cwd, empty);
        };
        let Some(target) = find_leaf(&mut tab.tree, &tab.focus) else {
            return false;
        };
        let added = leaf(cwd, empty);
        let focus = added.id.clone();
        *target = Node::Split {
            id: identity(),
            direction,
            ratio: 0.5,
            first: Box::new(target.clone()),
            second: Box::new(Node::Leaf(added)),
        };
        tab.focus = focus;
        self.focus = false;
        true
    }

    /// Change only the focused leaf's attachment reference and end its chooser
    /// state. Names, cwd and the ordinary shell identity remain unchanged.
    /// The caller validates the session/socket reference before saving.
    pub fn attach(&mut self, attachment: Option<Attachment>) -> bool {
        let Some(pane) = self.pane_mut() else {
            return false;
        };
        pane.attachment = attachment;
        pane.empty = false;
        true
    }

    /// End the focused leaf's chooser state without starting a process.
    pub fn open_terminal(&mut self) -> bool {
        let Some(pane) = self.pane_mut() else {
            return false;
        };
        pane.empty = false;
        true
    }

    /// Remove the selected tab record and select the first remaining tab.
    /// No shell or attachment session is stopped by this model operation.
    pub fn close_tab(&mut self) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        if space.tabs.is_empty() {
            return false;
        }
        let index = space
            .tabs
            .iter()
            .position(|tab| tab.id == space.selected)
            .unwrap_or(0);
        space.tabs.remove(index);
        space.selected = space
            .tabs
            .first()
            .map(|tab| tab.id.clone())
            .unwrap_or_default();
        true
    }

    /// Remove the focused pane record and collapse its split. Removing the last
    /// pane closes the tab; otherwise the first remaining leaf receives focus.
    pub fn close_pane(&mut self) -> bool {
        let Some(tab) = self.tab_mut() else {
            return false;
        };
        if let Some(tree) = remove_leaf(tab.tree.clone(), &tab.focus) {
            tab.focus = tree
                .leaves()
                .first()
                .map(|leaf| leaf.id.clone())
                .unwrap_or_default();
            tab.tree = tree;
            true
        } else {
            self.close_tab()
        }
    }

    /// Cycle pane focus in split order, wrapping at either end.
    /// Returns false when the selected workspace has no tab or focus is invalid.
    pub fn next_pane(&mut self, offset: isize) -> bool {
        let Some(tab) = self.tab_mut() else {
            return false;
        };
        let leaves = tab.tree.leaves();
        let Some(index) = leaves.iter().position(|leaf| leaf.id == tab.focus) else {
            return false;
        };
        let count = leaves.len() as isize;
        let next = (index as isize + offset.rem_euclid(count)).rem_euclid(count) as usize;
        tab.focus = leaves[next].id.clone();
        true
    }
}

fn find_leaf<'a>(node: &'a mut Node, id: &str) -> Option<&'a mut Node> {
    match node {
        Node::Leaf(leaf) if leaf.id == id => Some(node),
        Node::Leaf(_) => None,
        Node::Split { first, second, .. } => find_leaf(first, id).or_else(|| find_leaf(second, id)),
    }
}

fn remove_leaf(node: Node, target: &str) -> Option<Node> {
    match node {
        Node::Leaf(ref leaf) if leaf.id == target => None,
        Node::Leaf(_) => Some(node),
        Node::Split {
            id,
            direction,
            ratio,
            first,
            second,
        } => match (remove_leaf(*first, target), remove_leaf(*second, target)) {
            (None, other) | (other, None) => other,
            (Some(first), Some(second)) => Some(Node::Split {
                id,
                direction,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn navigation_reordering_transfer_and_empty_workspace_deletion() {
        let mut model = Layout::initial("/tmp".into());
        let first_space = model.selected.clone();
        let first_tab = model.tab().unwrap().id.clone();
        assert!(model.add_tab(Some("Second tab".into()), "/tmp".into(), true));
        let moved = model.tab().unwrap().clone();
        assert!(model.move_tab(-1));
        assert_eq!(model.workspace().unwrap().tabs[0], moved);
        assert!(model.move_tab(isize::MAX));
        assert_eq!(model.workspace().unwrap().tabs[1], moved);
        assert!(model.next_tab(1));
        assert_eq!(model.tab().unwrap().id, first_tab);
        assert!(model.next_tab(-1));
        assert_eq!(model.tab().unwrap(), &moved);
        assert!(!model.choose_tab("missing"));
        assert!(!model.delete_workspace());
        assert!(!model.transfer_tab("missing"));
        assert!(!model.transfer_tab(&first_space));

        model.add_workspace("Second workspace".into());
        let second_space = model.selected.clone();
        assert!(!model.next_tab(1));
        assert!(!model.transfer_tab(&first_space));
        assert!(model.next_workspace(1));
        assert_eq!(model.selected, first_space);
        assert!(model.choose_tab(&moved.id));
        assert!(model.transfer_tab(&second_space));
        assert_eq!(model.selected, second_space);
        assert_eq!(model.tab().unwrap(), &moved);
        assert!(model.next_workspace(-1));
        assert_eq!(model.tab().unwrap().id, first_tab);
        assert!(!model.choose_workspace("missing"));
        assert!(model.choose_workspace(&second_space));
        assert!(!model.delete_workspace());
        assert!(model.close_tab());
        assert!(model.delete_workspace());
        assert_eq!(model.selected, first_space);
        assert!(model.close_tab());
        assert!(!model.delete_workspace());
        assert_eq!(model.workspaces.len(), 1);
    }

    #[test]
    fn workspace_tab_split_attachment_and_close_match_the_model_workflow() {
        let mut model = Layout::initial("/tmp".into());
        assert_eq!(model.workspace().unwrap().name, "Workspace 1");
        assert!(model.tab().unwrap().name.starts_with("Tab "));
        assert!(!model.pane().unwrap().empty);
        let original = model.pane().unwrap().clone();
        let tab_name = model.tab().unwrap().name.clone();
        let attached = Attachment {
            session: "worker".into(),
            source_socket: Some("/tmp/source.sock".into()),
        };
        assert!(model.attach(Some(attached.clone())));
        assert_eq!(model.tab().unwrap().name, tab_name);
        model.focus = true;
        assert!(model.split(Direction::Right, "/var/tmp".into(), true));
        assert!(!model.focus);
        assert!(model.pane().unwrap().empty);
        let added = model.pane().unwrap().id.clone();
        assert!(model.next_pane(-1));
        assert_eq!(model.pane().unwrap().id, original.id);
        assert_eq!(model.pane().unwrap().attachment, Some(attached));
        assert_eq!(model.pane().unwrap().cwd, original.cwd);
        assert!(model.next_pane(1));
        assert_eq!(model.pane().unwrap().id, added);
        assert!(model.open_terminal());
        assert!(!model.pane().unwrap().empty);
        assert!(model.close_pane());
        assert_eq!(model.pane().unwrap().id, original.id);
        assert!(matches!(model.tab().unwrap().tree, Node::Leaf(_)));
        assert!(model.attach(None));
        assert_eq!(model.pane().unwrap().attachment, None);
        assert_eq!(model.tab().unwrap().name, tab_name);

        model.add_workspace("Second".into());
        assert!(model.workspace().unwrap().tabs.is_empty());
        assert!(model.pane().is_none());
        assert!(!model.attach(None));
        assert!(model.split(Direction::Below, "/tmp".into(), true));
        assert!(model.pane().unwrap().empty);
        assert!(model.add_tab(Some("Named".into()), "/tmp".into(), true));
        assert_eq!(model.tab().unwrap().name, "Named");
        assert!(model.close_tab());
        assert!(model.close_pane());
        assert!(model.workspace().unwrap().selected.is_empty());
        assert!(!model.close_tab());

        let mut identities = HashSet::new();
        for space in &model.workspaces {
            assert!(identities.insert(space.id.clone()));
            for tab in &space.tabs {
                assert!(identities.insert(tab.id.clone()));
                for leaf in tab.tree.leaves() {
                    assert!(identities.insert(leaf.id.clone()));
                }
            }
        }
        assert!(identities.iter().all(|id| {
            id.len() == 12
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }));
    }
}
