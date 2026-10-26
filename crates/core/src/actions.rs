//! In-memory equivalents of the Python workspace model operations. These do
//! not persist records or start/stop processes. Rename operations normalize
//! proposed names; creation methods expect caller-validated values.

use crate::{Attachment, Direction, Layout, Leaf, Node, Tab, View, Workspace};
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

/// Workspace icon picker choices as (glyph, label), in the Python tool's order.
/// Choosing Number clears the icon instead of storing a numeric glyph.
pub const WORKSPACE_ICONS: &[(&str, &str)] = &[
    ("◆", "Diamond"),
    ("●", "Circle"),
    ("▲", "Triangle"),
    ("■", "Square"),
    ("★", "Star"),
    ("✦", "Spark"),
    ("◈", "Gem"),
    ("⚑", "Flag"),
    ("⌂", "House"),
    ("✎", "Pencil"),
    ("⚙", "Gear"),
    ("♪", "Note"),
];

/// The destination, attachment and view captured when a session chooser opens.
#[derive(Clone, Debug)]
pub struct AttachmentTarget {
    tab: String,
    pane: String,
    attachment: Option<Attachment>,
    view: Option<View>,
}

impl AttachmentTarget {
    /// Apply to the captured pane if its tab is still selected and its attachment
    /// and view have not changed. Moving focus within that tab does not redirect the choice.
    /// None returns to the ordinary shell reference. The caller first refreshes
    /// shared data and cancels on reconciliation conflict; this changes only the
    /// model, without connecting to a session or persisting anything.
    #[must_use = "a changed destination requires a fresh choice"]
    pub fn apply(&self, layout: &mut Layout, attachment: Option<Attachment>) -> bool {
        let unchanged = layout.tab().is_some_and(|tab| {
            tab.id == self.tab
                && tab.tree.leaves().iter().any(|pane| {
                    pane.id == self.pane
                        && pane.attachment == self.attachment
                        && pane.view == self.view
                })
        });
        unchanged && layout.choose_pane(&self.pane) && layout.attach(attachment)
    }
}

/// The identity and original name captured when an inline editor opens.
#[derive(Clone, Debug)]
pub struct NameEdit {
    workspace: String,
    tab: Option<String>,
    name: String,
}

/// How the caller should finish an inline name edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameOutcome {
    /// The name changed; close the editor.
    Applied,
    /// No printable name remains; keep editing.
    Empty,
    /// The selection, target or original name changed; cancel and ask to retry.
    Changed,
}

impl NameEdit {
    /// The original name to show selected in the editor.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Apply to the same selected identity only if its original name still matches.
    /// The caller first refreshes shared data and cancels on a reconciliation
    /// conflict. Empty input is reported before checking the target, matching the
    /// Python editor. This does not refresh, persist or change terminal processes.
    #[must_use = "empty names keep editing; changed targets cancel the editor"]
    pub fn apply(&self, layout: &mut Layout, input: &str) -> RenameOutcome {
        let Some(name) = rename_value(input) else {
            return RenameOutcome::Empty;
        };
        if !layout
            .workspace()
            .is_some_and(|space| space.id == self.workspace)
        {
            return RenameOutcome::Changed;
        }
        let target = if let Some(id) = &self.tab {
            layout
                .tab_mut()
                .filter(|tab| tab.id == *id)
                .map(|tab| &mut tab.name)
        } else {
            layout.workspace_mut().map(|space| &mut space.name)
        };
        match target {
            Some(target) if *target == self.name => {
                *target = name;
                RenameOutcome::Applied
            }
            _ => RenameOutcome::Changed,
        }
    }
}

fn rename_value(value: &str) -> Option<String> {
    // Python str.strip additionally treats these four separators as whitespace.
    let name: String = value
        .trim_matches(|ch: char| ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch))
        .chars()
        .filter(|ch| {
            *ch == ' '
                || !matches!(
                    ch.general_category_group(),
                    GeneralCategoryGroup::Other | GeneralCategoryGroup::Separator
                )
        })
        .take(80)
        .collect();
    (!name.is_empty()).then_some(name)
}

fn identity() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

fn leaf(cwd: String, empty: bool) -> Leaf {
    Leaf {
        id: identity(),
        attachment: None,
        cwd: Some(cwd),
        empty,
        view: None,
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
        expanded: false,
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
            symbol: None,
            selected: tab.id.clone(),
            tabs: vec![tab],
        };
        Self {
            selected: space.id.clone(),
            workspaces: vec![space],
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

    /// Whether the selected tab shows only its focused pane.
    pub fn focus_mode(&self) -> bool {
        self.tab().is_some_and(|tab| tab.expanded)
    }

    /// Change expansion only in the selected tab; an empty workspace is unchanged.
    pub fn set_focus_mode(&mut self, expanded: bool) {
        if let Some(tab) = self.tab_mut() {
            tab.expanded = expanded;
        }
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

    /// Bind an attachment chooser or Return to shell action to the focused pane.
    pub fn attachment_target(&self) -> Option<AttachmentTarget> {
        let tab = self.tab()?;
        let pane = self.pane()?;
        Some(AttachmentTarget {
            tab: tab.id.clone(),
            pane: pane.id.clone(),
            attachment: pane.attachment.clone(),
            view: pane.view.clone(),
        })
    }

    /// Fill the named empty pane in the selected tab, selecting that destination
    /// even if focus moved. None chooses its ordinary shell reference. A stale
    /// tab, missing pane or already filled pane leaves the model unchanged.
    /// The caller first refreshes shared data, cancels on reconciliation conflict
    /// and checks that the destination is displayed. This starts no process.
    #[must_use = "a changed destination requires a fresh choice"]
    pub fn fill_pane(
        &mut self,
        tab_id: &str,
        pane_id: &str,
        attachment: Option<Attachment>,
    ) -> bool {
        let empty = self.tab().is_some_and(|tab| {
            tab.id == tab_id
                && tab
                    .tree
                    .leaves()
                    .iter()
                    .any(|pane| pane.id == pane_id && pane.empty && pane.attachment.is_none())
        });
        if !empty || !self.choose_pane(pane_id) {
            return false;
        }
        match attachment {
            Some(attachment) => self.attach(Some(attachment)),
            None => self.open_terminal(),
        }
    }

    /// Show a view in the named empty pane of the selected tab, selecting that
    /// destination. The same guards as `fill_pane` apply; an agent view needs
    /// an agent and every other view has none. This starts no process.
    #[must_use = "a changed destination requires a fresh choice"]
    pub fn show_view(&mut self, tab_id: &str, pane_id: &str, view: View) -> bool {
        let empty = self.tab().is_some_and(|tab| {
            tab.id == tab_id
                && tab
                    .tree
                    .leaves()
                    .iter()
                    .any(|pane| pane.id == pane_id && pane.empty && pane.attachment.is_none())
        });
        if !empty || !view.is_valid() || !self.choose_pane(pane_id) {
            return false;
        }
        let Some(pane) = self.pane_mut() else {
            return false;
        };
        pane.empty = false;
        pane.view = Some(view);
        true
    }

    /// Capture the current workspace's identity and name for a guarded inline edit.
    pub fn edit_workspace_name(&self) -> Option<NameEdit> {
        let space = self.workspace()?;
        Some(NameEdit {
            workspace: space.id.clone(),
            tab: None,
            name: space.name.clone(),
        })
    }

    /// Capture the current tab and workspace identities and the tab's original name.
    pub fn edit_tab_name(&self) -> Option<NameEdit> {
        let space = self.workspace()?;
        let tab = self.tab()?;
        Some(NameEdit {
            workspace: space.id.clone(),
            tab: Some(tab.id.clone()),
            name: tab.name.clone(),
        })
    }

    /// Rename the selected workspace, even when it has no tabs. Trim surrounding
    /// whitespace, remove non-printing characters and keep at most 80 Unicode
    /// scalar values, as in the Python name form. An empty result does nothing.
    /// This changes only the name; persistence validation remains the caller's job.
    pub fn rename_workspace(&mut self, value: &str) -> bool {
        let (Some(name), Some(space)) = (rename_value(value), self.workspace_mut()) else {
            return false;
        };
        space.name = name;
        true
    }

    /// Set the selected workspace's named native icon, or clear it together
    /// with any older one-glyph icon so the workspace shows its number.
    /// Returns whether anything changed; names are validated by persistence.
    pub fn set_workspace_symbol(&mut self, symbol: Option<&str>) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        let cleared = symbol.is_none() && space.icon.take().is_some();
        if space.symbol.as_deref() == symbol {
            return cleared;
        }
        space.symbol = symbol.map(str::to_owned);
        true
    }

    /// Rename the selected tab using the workspace name rules. Missing tabs or
    /// empty results do nothing. Pane identities and attachments stay unchanged.
    pub fn rename_tab(&mut self, value: &str) -> bool {
        let (Some(name), Some(tab)) = (rename_value(value), self.tab_mut()) else {
            return false;
        };
        tab.name = name;
        true
    }

    /// Set the current workspace's icon, even when it has no tabs. None selects
    /// Number. A supplied value must be a validated one- or two-character icon;
    /// the picker uses [`WORKSPACE_ICONS`]. This changes no selection or shell.
    pub fn set_workspace_icon(&mut self, icon: Option<&str>) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        space.icon = icon.map(str::to_owned);
        true
    }

    /// Select a workspace by its one-based position in saved order. Zero or a
    /// missing position does nothing. Each workspace keeps its tab and pane focus.
    pub fn choose_workspace_number(&mut self, number: usize) -> bool {
        let Some(space) = number.checked_sub(1).and_then(|i| self.workspaces.get(i)) else {
            return false;
        };
        self.selected = space.id.clone();
        true
    }

    /// Select a tab by its one-based position in the current workspace. Zero or
    /// a missing position does nothing. This retains the tab's focused pane.
    pub fn choose_tab_number(&mut self, number: usize) -> bool {
        let Some(space) = self.workspace_mut() else {
            return false;
        };
        let Some(tab) = number.checked_sub(1).and_then(|i| space.tabs.get(i)) else {
            return false;
        };
        space.selected = tab.id.clone();
        true
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

    /// Select a leaf in the current tab. A stale or other tab's identity does
    /// nothing. This changes model focus only; the caller confirms the terminal
    /// input destination separately.
    pub fn choose_pane(&mut self, id: &str) -> bool {
        let Some(tab) = self.tab_mut() else {
            return false;
        };
        if !tab.tree.leaves().iter().any(|leaf| leaf.id == id) {
            return false;
        }
        tab.focus = id.to_owned();
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
            symbol: None,
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
        tab.expanded = false;
        true
    }

    /// Move an existing pane beside another in the selected tab. All leaf
    /// identities and contents survive; the old parent collapses and the new
    /// split shares its space equally. Invalid destinations change nothing.
    pub fn move_pane(
        &mut self,
        source: &str,
        destination: &str,
        direction: Direction,
        before: bool,
    ) -> bool {
        if source == destination {
            return false;
        }
        let Some(tab) = self.tab_mut() else {
            return false;
        };
        let leaves = tab.tree.leaves();
        let Some(leaf) = leaves.iter().find(|leaf| leaf.id == source) else {
            return false;
        };
        if !leaves.iter().any(|leaf| leaf.id == destination) {
            return false;
        }
        let moved = Node::Leaf((*leaf).clone());
        let Some(mut tree) = remove_leaf(tab.tree.clone(), source) else {
            return false;
        };
        let Some(target) = find_leaf(&mut tree, destination) else {
            return false;
        };
        let (first, second) = if before {
            (moved, target.clone())
        } else {
            (target.clone(), moved)
        };
        *target = Node::Split {
            id: identity(),
            direction,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        };
        tab.tree = tree;
        tab.focus = source.into();
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
        pane.view = None;
        true
    }

    /// End the focused leaf's chooser state or view without starting a process.
    pub fn open_terminal(&mut self) -> bool {
        let Some(pane) = self.pane_mut() else {
            return false;
        };
        pane.empty = false;
        pane.view = None;
        true
    }

    /// Clear the focused pane's contents and return it to the chooser.
    /// The pane identity and directory survive; no tmux session is stopped.
    pub fn detach_pane(&mut self) -> bool {
        let Some(pane) = self.pane_mut() else {
            return false;
        };
        if pane.empty {
            return false;
        }
        pane.attachment = None;
        pane.view = None;
        pane.empty = true;
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
    use crate::ViewKind;
    use std::collections::HashSet;

    #[test]
    fn moving_panes_preserves_contents_and_collapses_the_old_parent() {
        let mut model = Layout::initial("/tmp".into());
        model.attach(Some(Attachment {
            session: "fixture".into(),
            source_socket: None,
        }));
        let source = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/tmp".into(), true);
        let destination = model.pane().unwrap().id.clone();
        assert!(model.show_view(
            &model.tab().unwrap().id.clone(),
            &destination,
            View::of(ViewKind::Team),
        ));
        model.split(Direction::Below, "/tmp".into(), true);
        let third = model.pane().unwrap().id.clone();
        let original = model.clone();
        let contents: Vec<_> = original
            .tab()
            .unwrap()
            .tree
            .leaves()
            .into_iter()
            .cloned()
            .collect();
        for direction in [Direction::Right, Direction::Below] {
            for before in [false, true] {
                let mut moved = original.clone();
                assert!(moved.move_pane(&source, &destination, direction, before));
                let tab = moved.tab().unwrap();
                assert_eq!(tab.focus, source);
                assert_eq!(tab.tree.leaves().len(), 3);
                for leaf in &contents {
                    assert!(tab.tree.leaves().contains(&leaf));
                }
                let Node::Split { first, second, .. } = &tab.tree else {
                    panic!("outer split")
                };
                assert!(matches!(second.as_ref(), Node::Leaf(leaf) if leaf.id == third));
                let Node::Split {
                    direction: actual,
                    first,
                    second,
                    ..
                } = first.as_ref()
                else {
                    panic!("destination split")
                };
                assert_eq!(*actual, direction);
                assert_eq!(
                    first.leaves()[0].id,
                    if before { &source } else { &destination }.as_str()
                );
                assert_eq!(
                    second.leaves()[0].id,
                    if before { &destination } else { &source }.as_str()
                );
            }
        }
        assert!(!model.move_pane(&source, &source, Direction::Below, false));
        assert!(!model.move_pane("missing", &destination, Direction::Below, false));
        assert!(!model.move_pane(&source, "missing", Direction::Below, false));
        assert_eq!(model, original);
    }

    #[test]
    fn attachment_choices_keep_their_destination_and_refuse_changed_targets() {
        let mut model = Layout::initial("/tmp".into());
        let first = model.pane().unwrap().id.clone();
        let target = model.attachment_target().unwrap();
        let tab = model.tab().unwrap().clone();
        model.split(Direction::Right, "/tmp".into(), false);
        let second = model.pane().unwrap().id.clone();
        let attachment = Attachment {
            session: "work".into(),
            source_socket: Some("/tmp/source.sock".into()),
        };
        assert!(target.apply(&mut model, Some(attachment.clone())));
        assert_eq!(model.pane().unwrap().id, first);
        assert_eq!(model.tab().unwrap().name, tab.name);
        assert!(model.tab().unwrap().tree.leaves()[1].attachment.is_none());
        let changed = model.clone();
        assert!(!target.apply(&mut model, None));
        assert_eq!(model, changed);

        let target = model.attachment_target().unwrap();
        model.attach(Some(Attachment {
            source_socket: Some("/tmp/peer.sock".into()),
            ..attachment
        }));
        let changed = model.clone();
        assert!(!target.apply(&mut model, None));
        assert_eq!(model, changed);
        let target = model.attachment_target().unwrap();
        model.add_tab(None, "/tmp".into(), true);
        let changed = model.clone();
        assert!(!target.apply(&mut model, None));
        assert_eq!(model, changed);
        model.choose_tab(&tab.id);
        model.close_pane();
        let changed = model.clone();
        assert!(!target.apply(&mut model, None));
        assert_eq!(model, changed);
        assert_eq!(model.pane().unwrap().id, second);

        model.attach(Some(Attachment {
            session: "other".into(),
            source_socket: None,
        }));
        let target = model.attachment_target().unwrap();
        assert!(target.apply(&mut model, None));
        assert!(model.pane().unwrap().attachment.is_none());
        assert_eq!(model.pane().unwrap().id, second);
    }

    #[test]
    fn attachment_choices_refuse_a_view_replaced_by_a_terminal() {
        let mut model = Layout::initial("/tmp".into());
        model.split(Direction::Right, "/tmp".into(), true);
        let tab = model.tab().unwrap().id.clone();
        let pane = model.pane().unwrap().id.clone();
        assert!(model.show_view(&tab, &pane, View::of(ViewKind::Team)));
        let target = model.attachment_target().unwrap();
        let attachment = Attachment {
            session: "work".into(),
            source_socket: None,
        };
        assert!(target.apply(&mut model.clone(), Some(attachment.clone())));

        assert!(model.open_terminal());
        let changed = model.clone();
        assert!(!target.apply(&mut model, Some(attachment.clone())));
        assert_eq!(model, changed);
        let current = model.attachment_target().unwrap();
        assert!(current.apply(&mut model, Some(attachment)));
    }

    #[test]
    fn empty_choices_fill_only_the_named_unfilled_pane() {
        let mut model = Layout::initial("/tmp".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Below, "/tmp/project".into(), true);
        let tab = model.tab().unwrap().id.clone();
        let destination = model.pane().unwrap().id.clone();
        model.choose_pane(&first);
        let before = model.clone();
        assert!(!model.fill_pane("missing", &destination, None));
        assert!(!model.fill_pane(&tab, "missing", None));
        assert!(!model.fill_pane(&tab, &first, None));
        assert_eq!(model, before);
        let attachment = Attachment {
            session: "work".into(),
            source_socket: Some("/tmp/source.sock".into()),
        };
        assert!(model.fill_pane(&tab, &destination, Some(attachment.clone())));
        assert_eq!(model.pane().unwrap().id, destination);
        assert_eq!(model.pane().unwrap().attachment, Some(attachment));
        assert_eq!(model.pane().unwrap().cwd.as_deref(), Some("/tmp/project"));
        let filled = model.clone();
        assert!(!model.fill_pane(&tab, &destination, None));
        assert_eq!(model, filled);
        model.split(Direction::Right, "/tmp/project".into(), true);
        let destination = model.pane().unwrap().id.clone();
        assert!(model.fill_pane(&tab, &destination, None));
        assert!(!model.pane().unwrap().empty);
        assert!(model.pane().unwrap().attachment.is_none());
    }

    #[test]
    fn views_fill_only_empty_panes_and_name_an_agent_only_for_agent_views() {
        let mut model = Layout::initial("/tmp".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/tmp".into(), true);
        let tab = model.tab().unwrap().id.clone();
        let destination = model.pane().unwrap().id.clone();
        let usage = View::of(ViewKind::Usage);
        let before = model.clone();
        assert!(!model.show_view(&tab, &first, usage.clone()));
        for wrong in [
            View::of(ViewKind::Agent),
            View {
                agent: Some("worker".into()),
                ..View::of(ViewKind::Usage)
            },
            View {
                section: Some(crate::AgentSection::Skills),
                ..View::of(ViewKind::Skills)
            },
        ] {
            assert!(!model.show_view(&tab, &destination, wrong));
        }
        assert_eq!(model, before);
        assert!(model.show_view(&tab, &destination, usage.clone()));
        let pane = model.pane().unwrap();
        assert!(!pane.empty && !pane.runs_terminal());
        assert_eq!(pane.view, Some(usage.clone()));
        assert!(!model.show_view(&tab, &destination, usage));
        assert!(model.open_terminal());
        assert!(model.pane().unwrap().runs_terminal());
    }

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
    fn inline_rename_refuses_changed_targets_and_keeps_empty_input_open() {
        let mut model = Layout::initial("/tmp".into());
        let edit = model.edit_tab_name().unwrap();
        assert_eq!(edit.name(), model.tab().unwrap().name);
        let original = model.clone();
        assert_eq!(edit.apply(&mut model, " \n"), RenameOutcome::Empty);
        assert_eq!(model, original);
        model.rename_tab("Peer name");
        let refreshed = model.clone();
        assert_eq!(edit.apply(&mut model, "My name"), RenameOutcome::Changed);
        assert_eq!(model, refreshed);
        let edit = model.edit_tab_name().unwrap();
        model.add_tab(Some("Other".into()), "/tmp".into(), true);
        let switched = model.clone();
        assert_eq!(edit.apply(&mut model, "My name"), RenameOutcome::Changed);
        assert_eq!(model, switched);
        model.choose_tab(&refreshed.tab().unwrap().id);
        let edit = model.edit_tab_name().unwrap();
        assert_eq!(
            edit.apply(&mut model, "  My name  "),
            RenameOutcome::Applied
        );
        assert_eq!(model.tab().unwrap().name, "My name");
        let edit = model.edit_tab_name().unwrap();
        model.close_tab();
        let deleted = model.clone();
        assert_eq!(edit.apply(&mut model, "Deleted"), RenameOutcome::Changed);
        assert_eq!(model, deleted);
        let edit = model.edit_workspace_name().unwrap();
        model.add_workspace("Empty".into());
        let switched = model.clone();
        assert_eq!(
            edit.apply(&mut model, "Wrong workspace"),
            RenameOutcome::Changed
        );
        assert_eq!(model, switched);
        assert!(model.edit_tab_name().is_none());
        let edit = model.edit_workspace_name().unwrap();
        assert_eq!(
            edit.apply(&mut model, "Renamed empty"),
            RenameOutcome::Applied
        );
        assert_eq!(model.workspace().unwrap().name, "Renamed empty");
    }

    #[test]
    fn renaming_normalizes_names_and_preserves_terminal_identity() {
        let mut model = Layout::initial("/tmp".into());
        model.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: None,
        }));
        model.split(Direction::Right, "/tmp".into(), true);
        let original = model.clone();
        assert!(model.rename_tab("\u{1c}  Build\n\u{200b} logs  \u{1f}"));
        let mut expected = original.clone();
        expected.tab_mut().unwrap().name = "Build logs".into();
        assert_eq!(model, expected);
        assert!(!model.rename_tab(" \t\n\u{200b} "));
        assert_eq!(model, expected);
        assert!(model.rename_tab(&"é".repeat(81)));
        assert_eq!(model.tab().unwrap().name, "é".repeat(80));
        model.add_workspace("Empty".into());
        let empty = model.clone();
        assert!(!model.rename_tab("Unavailable"));
        assert!(!model.rename_workspace("\u{1c} \u{1f}"));
        assert_eq!(model, empty);
        assert!(model.rename_workspace("  Renamed  "));
        let mut expected = empty;
        expected.workspace_mut().unwrap().name = "Renamed".into();
        assert_eq!(model, expected);
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
        model.set_focus_mode(true);
        assert!(model.split(Direction::Right, "/var/tmp".into(), true));
        assert!(!model.focus_mode());
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

    #[test]
    fn numbered_choices_and_icons_preserve_layouts_and_focus() {
        let mut model = Layout::initial("/tmp".into());
        model.split(Direction::Right, "/tmp".into(), false);
        let first_tab = model.tab().unwrap().clone();
        model.add_tab(Some("Second".into()), "/tmp".into(), true);
        let second_tab = model.tab().unwrap().clone();
        model.add_workspace("Empty".into());
        let empty_workspace = model.selected.clone();
        let before = model.clone();
        assert!(!model.choose_tab_number(1));
        for number in [0, 3, usize::MAX] {
            assert!(!model.choose_workspace_number(number));
            assert!(!model.choose_tab_number(number));
        }
        assert_eq!(model, before);
        for &(glyph, _) in WORKSPACE_ICONS {
            assert!(model.set_workspace_icon(Some(glyph)));
            assert_eq!(model.workspace().unwrap().icon.as_deref(), Some(glyph));
        }
        assert!(model.set_workspace_icon(None));
        assert_eq!(model, before);
        assert!(model.set_workspace_icon(Some("◆")));
        assert!(model.choose_workspace_number(1));
        assert_eq!(model.tab().unwrap(), &second_tab);
        assert!(model.workspace().unwrap().icon.is_none());
        assert!(model.choose_tab_number(1));
        assert_eq!(model.tab().unwrap(), &first_tab);
        let before = model.clone();
        for number in [0, 3, usize::MAX] {
            assert!(!model.choose_tab_number(number));
        }
        assert_eq!(model, before);
        assert!(model.choose_workspace_number(2));
        assert_eq!(model.selected, empty_workspace);
        assert_eq!(model.workspace().unwrap().icon.as_deref(), Some("◆"));
        assert!(model.choose_workspace_number(1));
        assert_eq!(model.tab().unwrap(), &first_tab);
        model.workspaces.clear();
        let empty = model.clone();
        assert!(!model.set_workspace_icon(None));
        assert!(!model.choose_workspace_number(1));
        assert!(!model.choose_tab_number(1));
        assert_eq!(model, empty);
    }
}
