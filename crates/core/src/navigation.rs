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
    /// Select an existing attachment without creating or attaching a session.
    ///
    /// Prefer the current tab, then the tab with the highest window-local use
    /// counter, then its focused matching pane. Unvisited tabs rank below any
    /// visited tab; remaining ties keep workspace, tab and split order.
    /// `source_matches` compares the attachment's source with this window's
    /// canonical source socket, treating an absent source as the window default.
    /// Path resolution belongs to the caller; this model performs no I/O.
    ///
    /// The caller refreshes the shared arrangement before this operation and
    /// confirms the selected attachment again before handing off keyboard input.
    /// False means no match and leaves the layout unchanged, so the caller can
    /// show session status. Recency is owned by the window and is not persisted.
    pub fn choose_attached_session(
        &mut self,
        name: &str,
        tab_uses: &HashMap<String, u64>,
        source_matches: impl Fn(Option<&str>) -> bool,
    ) -> bool {
        let current = self.tab().map(|tab| &tab.id);
        let mut best_rank = None;
        let mut target = None;
        for (space_index, space) in self.workspaces.iter().enumerate() {
            for (tab_index, tab) in space.tabs.iter().enumerate() {
                for pane in tab.tree.leaves() {
                    let Some(attachment) = &pane.attachment else {
                        continue;
                    };
                    if attachment.session != name
                        || !source_matches(attachment.source_socket.as_deref())
                    {
                        continue;
                    }
                    let rank = (
                        Some(&tab.id) == current,
                        tab_uses.get(&tab.id).copied(),
                        pane.id == tab.focus,
                    );
                    if best_rank.is_none_or(|best| rank > best) {
                        best_rank = Some(rank);
                        target = Some((space_index, tab_index, pane.id.clone()));
                    }
                }
            }
        }
        let Some((space_index, tab_index, pane)) = target else {
            return false;
        };
        let space = &mut self.workspaces[space_index];
        let tab = &mut space.tabs[tab_index];
        self.selected = space.id.clone();
        space.selected = tab.id.clone();
        tab.focus = pane;
        true
    }

    /// Keep this window's navigation when loading a newer shared arrangement.
    ///
    /// Existing selections follow stable identities, including pane focus when
    /// a tab moves to a different workspace. A removed selection falls back to
    /// the first available item; an empty workspace selects no tab. Shared
    /// names, trees, attachments and roster visibility come from `self`.
    /// Returns true when the active tab's tree changed, including a change of
    /// attachment or removal of the active tab. The caller must then move input
    /// focus to navigation and require a fresh pane selection. Renaming alone
    /// or changing saved working directories does not require that handoff.
    /// This does not dispatch input, write either
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
        match (self.tab(), previous.tab()) {
            (Some(current), Some(old)) => !same_input_tree(&current.tree, &old.tree),
            (None, None) => false,
            _ => true,
        }
    }
}

// Saved cwd is metadata for an ordinary shell, not its input destination.
fn same_input_tree(current: &Node, previous: &Node) -> bool {
    match (current, previous) {
        (Node::Leaf(a), Node::Leaf(b)) => {
            a.id == b.id && a.attachment == b.attachment && a.empty == b.empty
        }
        (
            Node::Split {
                id: a_id,
                direction: a_direction,
                ratio: a_ratio,
                first: a_first,
                second: a_second,
            },
            Node::Split {
                id: b_id,
                direction: b_direction,
                ratio: b_ratio,
                first: b_first,
                second: b_second,
            },
        ) => {
            a_id == b_id
                && a_direction == b_direction
                && a_ratio == b_ratio
                && same_input_tree(a_first, b_first)
                && same_input_tree(a_second, b_second)
        }
        _ => false,
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
    fn session_navigation_prefers_current_recent_and_focused_matches_on_its_source() {
        let mut model = Layout::initial("/tmp".into());
        let first_space = model.selected.clone();
        let first_tab = model.tab().unwrap().id.clone();
        let first_pane = model.pane().unwrap().id.clone();
        let local = Attachment {
            session: "worker".into(),
            source_socket: None,
        };
        model.attach(Some(local.clone()));
        model.split(Direction::Right, "/tmp".into(), true);
        model.attach(Some(Attachment {
            source_socket: Some("/fixture/source".into()),
            ..local.clone()
        }));
        let focused_pane = model.pane().unwrap().id.clone();
        model.add_workspace("Other workspace".into());
        model.add_tab(None, "/tmp".into(), true);
        model.attach(Some(local.clone()));
        let older_tab = model.tab().unwrap().id.clone();
        model.add_tab(None, "/tmp".into(), true);
        model.attach(Some(local.clone()));
        let recent_tab = model.tab().unwrap().id.clone();
        let uses = HashMap::from([(older_tab.clone(), 0), (recent_tab.clone(), 1)]);
        let source_matches =
            |source: Option<&str>| source.is_none() || source == Some("/fixture/source");

        model.choose_workspace(&first_space);
        assert!(model.choose_attached_session("worker", &uses, source_matches));
        assert_eq!(model.tab().unwrap().id, first_tab);
        assert_eq!(model.pane().unwrap().id, focused_pane);

        // A matching name on another server cannot win, even when focused.
        model.attach(Some(Attachment {
            source_socket: Some("/fixture/other".into()),
            ..local
        }));
        assert!(model.choose_attached_session("worker", &uses, source_matches));
        assert_eq!(model.pane().unwrap().id, first_pane);
        model.attach(None);
        assert!(model.choose_attached_session("worker", &uses, source_matches));
        assert_eq!(model.tab().unwrap().id, recent_tab);
        assert_ne!(model.selected, first_space);

        // A visited tab with counter zero outranks an unvisited tab.
        model.choose_workspace(&first_space);
        let uses = HashMap::from([(recent_tab.clone(), 0)]);
        assert!(model.choose_attached_session("worker", &uses, source_matches));
        assert_eq!(model.tab().unwrap().id, recent_tab);

        // Equal unvisited ranks retain saved order across tabs.
        model.choose_workspace(&first_space);
        assert!(model.choose_attached_session("worker", &HashMap::new(), source_matches));
        assert_eq!(model.tab().unwrap().id, older_tab);

        let before = model.clone();
        assert!(!model.choose_attached_session("missing", &uses, source_matches));
        assert_eq!(model, before);
        assert!(!model.choose_attached_session("worker", &uses, |_| false));
        assert_eq!(model, before);
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
                    symbol: None,
                    selected: tab.id.clone(),
                    tabs: vec![tab],
                },
                Workspace {
                    id: "000000000006".into(),
                    name: "Second".into(),
                    icon: None,
                    symbol: None,
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
        if let Node::Leaf(leaf) = &mut incoming.workspaces[0].tabs[0].tree {
            leaf.cwd = Some("/changed/directory".into());
        }
        assert!(!incoming.retain_navigation(&previous));
        assert_eq!(
            incoming.pane().unwrap().cwd.as_deref(),
            Some("/changed/directory")
        );
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
