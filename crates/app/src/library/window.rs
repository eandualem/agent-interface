use std::{collections::HashMap, io, path::Path};

use tmux_workspaces_core::{Layout, Leaf};
use tmux_workspaces_store::{Document, Error, Reconciliation};

/// One window's local model and the shared document on which its edits are based.
///
/// Stored navigation seeds this model only once. Later library reads preserve
/// local selection and reconcile structural edits through the store's conflict
/// rules. Session selection reads socket paths without connecting to a server.
/// Database I/O remains on the storage worker, outside this state holder.
#[derive(Debug)]
pub struct WindowLayout {
    base: Document,
    model: Layout,
    tab_uses: HashMap<String, u64>,
    use_clock: u64,
    terminal_focused: bool,
}

impl WindowLayout {
    /// Seed a window from its first successful validated library read.
    pub fn new(document: Document) -> Self {
        Self {
            model: document.model().clone(),
            base: document,
            tab_uses: HashMap::new(),
            use_clock: 0,
            terminal_focused: false,
        }
    }

    /// This window's current arrangement and navigation.
    pub fn model(&self) -> &Layout {
        &self.model
    }

    /// Apply local model operations. Changes are in memory only and must remain
    /// schema-valid; an invalid model is rejected by the next refresh.
    /// Editing returns input focus to navigation. After displaying the resulting
    /// selection, explicitly focus its terminal if that is the user's intent.
    pub fn model_mut(&mut self) -> &mut Layout {
        self.focus_navigation();
        &mut self.model
    }

    /// Capture a save without borrowing the UI state during database I/O.
    /// The caller permits navigation, but no further edits, until completion.
    pub fn save_snapshot(&self) -> (Document, Layout) {
        (self.base.clone(), self.model.clone())
    }

    /// Apply a completed save or conflict refresh, keeping navigation performed
    /// while the worker was busy. `base` is the base updated by `Store::save`.
    /// Returns whether input must return to navigation before further typing.
    pub fn saved(&mut self, base: Document, result: &Reconciliation) -> bool {
        let mut model = result.document.model().clone();
        let changed = model.retain_navigation(&self.model);
        self.base = base;
        self.model = model;
        changed
    }

    /// Abandon the unsaved edit without touching the database or local navigation.
    /// The caller resumes polling to reconcile any newer shared changes.
    pub fn discard_edits(&mut self) -> bool {
        let mut model = self.base.model().clone();
        let changed = model.retain_navigation(&self.model);
        self.model = model;
        changed
    }

    /// The selected pane eligible for terminal input, or None while navigation
    /// has focus. This does not establish that a connection is ready or send keys.
    pub fn terminal_input(&self) -> Option<&Leaf> {
        self.model
            .pane()
            .filter(|pane| self.terminal_focused && pane.runs_terminal())
    }

    /// Explicitly move input focus to the selected terminal. The caller must
    /// first check that it is displayed and ready. An empty workspace or chooser
    /// stays in navigation. This state is local and never saved in layouts.db.
    pub fn focus_terminal(&mut self) -> bool {
        self.terminal_focused = self.model.pane().is_some_and(Leaf::runs_terminal);
        self.terminal_focused
    }

    /// Return input focus to navigation without changing pane selection or the
    /// separate display mode that shows only one pane.
    pub fn focus_navigation(&mut self) {
        self.terminal_focused = false;
    }

    /// Record the current tab only after it has actually been shown. Selecting
    /// a tab or applying a library refresh alone does not make it recently viewed.
    /// Empty workspaces do nothing. History stays local for this window's lifetime
    /// and is not saved with the layout, including history of temporarily removed IDs.
    pub fn record_shown_tab(&mut self) {
        if let Some(tab) = self.model.tab() {
            self.use_clock = self.use_clock.saturating_add(1);
            self.tab_uses.insert(tab.id.clone(), self.use_clock);
        }
    }

    /// Choose an existing attachment using this window's actual viewing history.
    /// Refresh the shared arrangement first and pass the captured window source.
    /// This reads source paths only and changes model selection, not terminal input.
    /// A selected attachment or a lookup error returns input focus to navigation.
    /// No match leaves the current focus unchanged.
    /// Call [`Self::record_shown_tab`] after the chosen tab is actually displayed.
    pub fn choose_attached_session(&mut self, name: &str, source: &Path) -> io::Result<bool> {
        let result =
            crate::sessions::choose_attached_session(&mut self.model, name, &self.tab_uses, source);
        if !matches!(&result, Ok(false)) {
            self.focus_navigation();
        }
        result
    }

    /// Apply a validated library replacement and return its conflict/input flags.
    ///
    /// The returned document's model is already applied here. The merge base
    /// advances to `latest`, not to the merged result, so the next refresh still
    /// recognizes unsaved local edits. A conflict adopts the latest arrangement
    /// and requires the caller to ask for a retry. An error leaves both this model
    /// and its merge base unchanged. `path` is used only for error diagnostics.
    /// An active-tree change or conflict returns input focus to navigation. A
    /// subsequent unchanged refresh cannot restore it; require an explicit choice.
    pub fn refresh(&mut self, latest: Document, path: &Path) -> Result<Reconciliation, Error> {
        let result = self.base.reconcile(&self.model, &latest, path)?;
        self.model = result.document.model().clone();
        self.base = latest;
        if result.active_tree_changed || result.conflict {
            self.focus_navigation();
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_workspaces_core::Attachment;

    #[test]
    fn discarding_an_edit_preserves_navigation_and_the_shared_base() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        let document = Document::parse(raw.into(), path).unwrap();
        let mut model = document.model().clone();
        model.add_tab(Some("First".into()), "/tmp".into(), true);
        let first = model.tab().unwrap().id.clone();
        model.add_tab(Some("Second".into()), "/tmp".into(), true);
        let mut window = WindowLayout::new(document.with_model(&model, path).unwrap());
        window.model_mut().rename_workspace("Unsaved");
        window.model_mut().choose_tab(&first);
        assert!(!window.discard_edits());
        assert_eq!(window.model().workspace().unwrap().name, "Workspace");
        assert_eq!(window.model().tab().unwrap().id, first);
        let mut latest = window.base.model().clone();
        latest.rename_workspace("Peer name");
        let result = window
            .refresh(window.base.with_model(&latest, path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict && !result.active_tree_changed);
        assert_eq!(window.model().workspace().unwrap().name, "Peer name");
    }

    #[test]
    fn a_peer_directory_snapshot_keeps_terminal_input_focus() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Tab","tree":{"id":"000000000003","agent":null,"cwd":"/before"},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        let latest = Document::parse(raw.replace("/before", "/after"), path).unwrap();
        let result = window.refresh(latest, path).unwrap();
        assert!(!result.active_tree_changed && !result.conflict);
        assert_eq!(
            window.model().pane().unwrap().cwd.as_deref(),
            Some("/after")
        );
    }

    #[test]
    fn save_completion_keeps_new_navigation_and_advances_the_edit_base() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layouts.db");
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer
            .execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            .unwrap();
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        let original = Document::parse(raw.into(), &path).unwrap();
        let mut model = original.model().clone();
        model.add_tab(Some("First".into()), "/tmp".into(), true);
        let first = model.tab().unwrap().id.clone();
        model.add_tab(Some("Second".into()), "/tmp".into(), true);
        let second = model.tab().unwrap().id.clone();
        let document = original.with_model(&model, &path).unwrap();
        writer
            .execute(
                "INSERT INTO terminal_layout VALUES (1, ?1)",
                [document.raw_json()],
            )
            .unwrap();
        let mut store =
            tmux_workspaces_store::Store::open_existing_writable(directory.path()).unwrap();
        let mut window = WindowLayout::new(document);
        window.model_mut().rename_tab("Renamed");
        let (mut base, local) = window.save_snapshot();
        // Selection changes while the database worker is in flight.
        window.model_mut().choose_tab(&first);
        let result = store.save(&mut base, &local).unwrap();
        assert!(!window.saved(base, &result));
        assert_eq!(window.model().tab().unwrap().id, first);
        assert_eq!(window.model().workspace().unwrap().tabs[1].name, "Renamed");
        let result = window.refresh(store.load().unwrap(), &path).unwrap();
        assert!(!result.conflict && !result.active_tree_changed);

        window.model_mut().choose_tab(&second);
        window.model_mut().rename_tab("Local retry");
        let (mut base, local) = window.save_snapshot();
        let mut peer = store.load().unwrap();
        let mut peer_model = peer.model().clone();
        peer_model.choose_tab(&second);
        peer_model.rename_tab("Peer name");
        assert!(!store.save(&mut peer, &peer_model).unwrap().conflict);
        let result = store.save(&mut base, &local).unwrap();
        assert!(result.conflict);
        assert!(!window.saved(base, &result));
        assert_eq!(window.model().tab().unwrap().name, "Peer name");
        window.model_mut().rename_tab("Retried");
        let (mut base, local) = window.save_snapshot();
        let result = store.save(&mut base, &local).unwrap();
        assert!(!result.conflict);
        window.saved(base, &result);
        assert_eq!(store.load().unwrap().model().tab().unwrap().name, "Retried");

        // A peer removes the selected tab during the final rename save.
        window.model_mut().rename_workspace("Updated");
        let (mut base, local) = window.save_snapshot();
        let mut peer = store.load().unwrap();
        let mut peer_model = peer.model().clone();
        peer_model.close_tab();
        assert!(!store.save(&mut peer, &peer_model).unwrap().conflict);
        let result = store.save(&mut base, &local).unwrap();
        assert!(window.saved(base, &result));
        assert_eq!(window.model().tab().unwrap().id, first);
    }

    #[test]
    fn failed_session_navigation_disarms_input_without_changing_the_layout() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Editor","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        let before = window.model().clone();
        assert!(window.focus_terminal());
        let error = window
            .choose_attached_session("worker", Path::new("invalid\0source"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(window.terminal_input().is_none());
        assert_eq!(window.model(), &before);

        // An unchanged library refresh cannot silently re-enable terminal input.
        let result = window
            .refresh(Document::parse(raw.into(), path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict && !result.active_tree_changed);
        assert!(window.terminal_input().is_none());
        assert!(window.focus_terminal());
        assert!(
            !window
                .choose_attached_session("missing", Path::new("/fixture/source"))
                .unwrap()
        );
        assert!(window.terminal_input().is_some());
    }

    #[test]
    fn changed_shared_panes_require_explicit_window_local_input_focus() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Editor","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":true}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        let mut other = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        assert!(window.terminal_input().is_none());
        assert!(window.model().focus_mode()); // Display zoom is not keyboard focus.
        assert!(window.focus_terminal());
        assert!(other.focus_terminal());
        assert_eq!(window.terminal_input().unwrap().id, "000000000003");

        let mut peer = base.model().clone();
        peer.rename_tab("Renamed");
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.active_tree_changed && !result.conflict);
        assert!(window.terminal_input().is_some());

        peer.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: None,
        }));
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.active_tree_changed);
        assert!(window.terminal_input().is_none());
        assert!(other.terminal_input().is_some());
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.active_tree_changed);
        assert!(window.terminal_input().is_none());
        assert!(window.focus_terminal());
        assert_eq!(
            window
                .terminal_input()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "worker"
        );
        assert!(
            window
                .choose_attached_session("worker", Path::new("/fixture/source"))
                .unwrap()
        );
        assert!(window.terminal_input().is_none());

        window.model_mut().rename_tab("Local name");
        assert!(window.focus_terminal());
        peer.rename_tab("Peer name");
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.conflict && !result.active_tree_changed);
        assert!(window.terminal_input().is_none());

        peer.split(tmux_workspaces_core::Direction::Right, "/tmp".into(), false);
        let second = peer.pane().unwrap().id.clone();
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.active_tree_changed && !result.conflict);
        window.model_mut().choose_pane(&second);
        assert!(window.focus_terminal());
        assert_eq!(window.terminal_input().unwrap().id, second);
        // A replacement tab can retain the same tree while its new identity
        // causes navigation to fall back to the first leaf.
        peer.workspaces[0].tabs[0].id = "aabbccddeeff".into();
        peer.workspaces[0].selected = "aabbccddeeff".into();
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict);
        assert!(result.active_tree_changed);
        assert_ne!(window.model().pane().unwrap().id, second);
        assert!(window.terminal_input().is_none());

        assert!(window.focus_terminal());
        window.focus_navigation();
        assert!(window.terminal_input().is_none());
        assert!(!window.model().focus_mode()); // A replacement tab starts unexpanded.
        assert!(window.focus_terminal());
        window.model_mut().add_tab(None, "/tmp".into(), true);
        assert!(window.terminal_input().is_none());
        assert!(!window.focus_terminal()); // The new pane is a chooser.
        while window.model().tab().is_some() {
            window.model_mut().close_tab();
        }
        assert!(!window.focus_terminal());
        assert!(window.terminal_input().is_none());
    }

    #[test]
    fn repeated_refreshes_retain_local_edits_and_advance_the_base_only_after_success() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Editor","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        let other_window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        window
            .model_mut()
            .add_tab(Some("Local tab".into()), "/tmp".into(), true);
        let local_tab = window.model().tab().unwrap().id.clone();
        let mut peer = base.model().clone();

        for name in ["Peer 1", "Peer 2"] {
            peer.rename_workspace(name);
            let result = window
                .refresh(base.with_model(&peer, path).unwrap(), path)
                .unwrap();
            assert!(!result.conflict && !result.active_tree_changed);
            assert_eq!(window.model().workspace().unwrap().name, name);
            assert_eq!(window.model().tab().unwrap().id, local_tab);
            assert_eq!(window.model().workspace().unwrap().tabs.len(), 2);
        }
        assert_eq!(other_window.model(), base.model());

        window.model_mut().rename_workspace("Local workspace");
        peer.rename_workspace("Peer 3");
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.conflict && result.active_tree_changed);
        assert_eq!(window.model().workspace().unwrap().name, "Peer 3");
        assert_eq!(window.model().workspace().unwrap().tabs.len(), 1);

        peer.rename_workspace("Peer 4");
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict && !result.active_tree_changed);

        // A rejected local model must not advance the merge base behind it.
        window.model_mut().workspaces[0].name.clear();
        let before = window.model().clone();
        peer.rename_workspace("Peer 5");
        assert!(
            window
                .refresh(base.with_model(&peer, path).unwrap(), path)
                .is_err()
        );
        assert_eq!(window.model(), &before);
        window.model_mut().workspaces[0].name = "Peer 4".into();
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict);
        assert_eq!(window.model().workspace().unwrap().name, "Peer 5");

        peer.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: None,
        }));
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict && result.active_tree_changed);
    }

    #[test]
    fn session_navigation_uses_only_this_windows_shown_tabs_across_refreshes() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut model = base.model().clone();
        let mut tabs = Vec::new();
        for name in ["First", "Second", "Other"] {
            model.add_tab(Some(name.into()), "/tmp".into(), true);
            if name != "Other" {
                model.attach(Some(Attachment {
                    session: "worker".into(),
                    source_socket: None,
                }));
            }
            tabs.push(model.tab().unwrap().id.clone());
        }
        let mut window = WindowLayout::new(base.with_model(&model, path).unwrap());
        let mut other = WindowLayout::new(base.with_model(&model, path).unwrap());
        let original_shared = window.base.raw_json().to_owned();
        let source = Path::new("/fixture/source");
        window.model_mut().choose_tab(&tabs[0]);
        window.record_shown_tab();
        window.model_mut().choose_tab(&tabs[1]);
        window.record_shown_tab();
        // Merely selecting First does not make it newer than shown Second.
        window.model_mut().choose_tab(&tabs[0]);
        window.model_mut().choose_tab(&tabs[2]);
        assert!(window.choose_attached_session("worker", source).unwrap());
        assert_eq!(window.model().tab().unwrap().id, tabs[1]);
        assert!(other.choose_attached_session("worker", source).unwrap());
        assert_eq!(other.model().tab().unwrap().id, tabs[0]);
        assert_eq!(window.base.raw_json(), original_shared);

        window.model_mut().choose_tab(&tabs[2]);
        model.rename_workspace("Peer name");
        let result = window
            .refresh(base.with_model(&model, path).unwrap(), path)
            .unwrap();
        assert!(!result.conflict && !result.active_tree_changed);
        assert!(window.choose_attached_session("worker", source).unwrap());
        assert_eq!(window.model().tab().unwrap().id, tabs[1]);
        window.model_mut().choose_tab(&tabs[0]);
        window.record_shown_tab();
        window.model_mut().choose_tab(&tabs[2]);
        assert!(window.choose_attached_session("worker", source).unwrap());
        assert_eq!(window.model().tab().unwrap().id, tabs[0]);

        while window.model().tab().is_some() {
            window.model_mut().close_tab();
        }
        let clock = window.use_clock;
        window.record_shown_tab();
        assert_eq!(window.use_clock, clock);
        assert!(!window.choose_attached_session("worker", source).unwrap());
    }
}
