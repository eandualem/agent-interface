use crate::{Layout, Leaf, Node};
use std::collections::HashMap;

impl Node {
    /// Leaves in split order: first child before second child.
    pub fn leaves(&self) -> Vec<&Leaf> {
        let mut pending = vec![self];
        let mut leaves = Vec::new();
        while let Some(node) = pending.pop() {
            match node {
                Node::Leaf(leaf) => leaves.push(leaf),
                Node::Split { first, second, .. } => {
                    pending.push(second);
                    pending.push(first);
                }
            }
        }
        leaves
    }
}

impl Layout {
    /// Keep this window's navigation when loading a newer shared arrangement.
    ///
    /// Existing selections follow stable identities, including pane focus when
    /// a tab moves to a different workspace. A removed selection falls back to
    /// the first available item; an empty workspace selects no tab. Shared
    /// names, trees, attachments and roster visibility come from `self`.
    /// Returns true when the active tab's tree changed, including a change of
    /// attachment or removal of the active tab. The caller must then move input
    /// focus to navigation and require a fresh pane selection. Renaming alone
    /// does not require that handoff. This does not dispatch input, write either
    /// arrangement or create a terminal session.
    #[must_use = "a changed active tree requires input focus to return to navigation"]
    pub fn retain_navigation(&mut self, previous: &Layout) -> bool {
        let spaces: HashMap<_, _> = previous
            .workspaces
            .iter()
            .map(|space| (&space.id, space))
            .collect();
        let tabs: HashMap<_, _> = previous
            .workspaces
            .iter()
            .flat_map(|space| &space.tabs)
            .map(|tab| (&tab.id, tab))
            .collect();
        for space in &mut self.workspaces {
            let selected = spaces.get(&space.id).map(|old| &old.selected);
            space.selected = space
                .tabs
                .iter()
                .find(|tab| Some(&tab.id) == selected)
                .or_else(|| space.tabs.first())
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
            for tab in &mut space.tabs {
                let leaves = tab.tree.leaves();
                let focus = tabs.get(&tab.id).map(|old| &old.focus);
                tab.focus = leaves
                    .iter()
                    .find(|leaf| Some(&leaf.id) == focus)
                    .or_else(|| leaves.first())
                    .map(|leaf| leaf.id.clone())
                    .unwrap_or_default();
            }
        }
        self.selected = self
            .workspaces
            .iter()
            .find(|space| space.id == previous.selected)
            .or_else(|| self.workspaces.first())
            .map(|space| space.id.clone())
            .unwrap_or_default();
        self.focus = previous.focus;
        self.tab().map(|tab| &tab.tree) != previous.tab().map(|tab| &tab.tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attachment, Direction, Tab, Workspace};

    fn leaf(id: &str) -> Node {
        Node::Leaf(Leaf {
            id: id.into(),
            attachment: None,
            cwd: None,
            empty: false,
        })
    }

    #[test]
    fn reload_retains_local_selection_across_moves_and_handles_deletions() {
        let tab = Tab {
            id: "000000000002".into(),
            name: "Editor".into(),
            focus: "000000000005".into(),
            tree: Node::Split {
                id: "000000000003".into(),
                direction: Direction::Right,
                ratio: 0.5,
                first: Box::new(leaf("000000000004")),
                second: Box::new(leaf("000000000005")),
            },
        };
        let previous = Layout {
            workspaces: vec![
                Workspace {
                    id: "000000000001".into(),
                    name: "First".into(),
                    icon: None,
                    selected: tab.id.clone(),
                    tabs: vec![tab],
                },
                Workspace {
                    id: "000000000006".into(),
                    name: "Second".into(),
                    icon: None,
                    selected: String::new(),
                    tabs: vec![],
                },
            ],
            selected: "000000000001".into(),
            focus: true,
            show_agents: false,
        };
        let mut incoming = previous.clone();
        let mut moved = incoming.workspaces[0].tabs.remove(0);
        moved.name = "Renamed remotely".into();
        moved.focus = "000000000004".into();
        incoming.workspaces[1].tabs.push(moved);
        incoming.selected = "000000000006".into();
        incoming.show_agents = true;
        incoming.focus = false;
        assert!(incoming.retain_navigation(&previous));
        assert_eq!(incoming.selected, "000000000001");
        assert!(incoming.workspaces[0].selected.is_empty());
        assert_eq!(incoming.workspaces[1].selected, "000000000002");
        assert_eq!(incoming.workspaces[1].tabs[0].focus, "000000000005");
        assert_eq!(incoming.workspaces[1].tabs[0].name, "Renamed remotely");
        assert!(incoming.focus && incoming.show_agents);
        assert_eq!(previous.workspaces[0].tabs[0].name, "Editor");

        let previous = incoming.clone();
        incoming.workspaces.remove(0);
        incoming.workspaces[0].tabs[0].tree = Node::Leaf(Leaf {
            id: "000000000004".into(),
            attachment: Some(Attachment {
                session: "worker".into(),
                source_socket: None,
            }),
            cwd: None,
            empty: false,
        });
        assert!(incoming.retain_navigation(&previous));
        assert_eq!(incoming.selected, "000000000006");
        assert_eq!(incoming.workspaces[0].tabs[0].focus, "000000000004");
        assert_eq!(
            incoming.workspaces[0].tabs[0].tree.leaves()[0]
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "worker"
        );
    }

    #[test]
    fn refresh_requires_pane_reselection_only_when_the_active_tree_changes() {
        let previous = Layout::initial("/tmp".into());
        let mut incoming = previous.clone();
        incoming.workspaces[0].name = "Renamed workspace".into();
        incoming.workspaces[0].tabs[0].name = "Renamed tab".into();
        incoming.show_agents = true;
        incoming.add_workspace("Peer workspace".into());
        incoming.add_tab(None, "/tmp".into(), true);
        assert!(!incoming.retain_navigation(&previous));
        assert_eq!(incoming.selected, previous.selected);

        let previous = incoming.clone();
        incoming.attach(Some(Attachment {
            session: "editor".into(),
            source_socket: None,
        }));
        assert!(incoming.retain_navigation(&previous));
        assert!(!incoming.clone().retain_navigation(&incoming));

        let previous = incoming.clone();
        incoming.split(Direction::Right, "/tmp".into(), true);
        assert!(incoming.retain_navigation(&previous));
        assert_eq!(incoming.tab().unwrap().focus, previous.tab().unwrap().focus);

        let previous = incoming.clone();
        incoming.close_tab();
        assert!(incoming.retain_navigation(&previous));
        assert!(incoming.tab().is_none());
        assert!(!incoming.clone().retain_navigation(&incoming));
    }
}
