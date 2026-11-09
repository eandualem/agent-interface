use std::path::Path;

use agent_interface_core::Layout;
use agent_interface_store::{Document, Error, Reconciliation};

/// One window's local model and the shared document on which its edits are based.
///
/// Stored navigation seeds this model only once. Later library reads preserve
/// local selection and reconcile structural edits through the store's conflict
/// rules. Database I/O remains on the storage worker, outside this state holder.
#[derive(Debug)]
pub struct WindowLayout {
    base: Document,
    model: Layout,
}

impl WindowLayout {
    /// Seed a window from its first successful validated library read.
    pub fn new(document: Document) -> Self {
        Self {
            model: document.model().clone(),
            base: document,
        }
    }

    /// This window's current arrangement and navigation.
    pub fn model(&self) -> &Layout {
        &self.model
    }

    /// Apply local model operations. Changes are in memory only and must remain
    /// schema-valid; an invalid model is rejected by the next refresh.
    pub fn model_mut(&mut self) -> &mut Layout {
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

    /// Apply a validated library replacement and return its conflict/input flags.
    ///
    /// The returned document's model is already applied here. The merge base
    /// advances to `latest`, not to the merged result, so the next refresh still
    /// recognizes unsaved local edits. A conflict adopts the latest arrangement
    /// and requires the caller to ask for a retry. An error leaves both this model
    /// and its merge base unchanged. `path` is used only for error diagnostics.
    pub fn refresh(&mut self, latest: Document, path: &Path) -> Result<Reconciliation, Error> {
        let result = self.base.reconcile(&self.model, &latest, path)?;
        self.model = result.document.model().clone();
        self.base = latest;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_core::Attachment;

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
            agent_interface_store::Store::open_existing_writable(directory.path()).unwrap();
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
    fn refreshes_report_shared_pane_changes_and_conflicts() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Editor","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":true}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());

        let mut peer = base.model().clone();
        peer.rename_tab("Renamed");
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.active_tree_changed && !result.conflict);

        peer.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: None,
        }));
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.active_tree_changed);
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!result.active_tree_changed);

        window.model_mut().rename_tab("Local name");
        peer.rename_tab("Peer name");
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.conflict && !result.active_tree_changed);

        peer.split(agent_interface_core::Direction::Right, "/tmp".into(), false);
        let second = peer.pane().unwrap().id.clone();
        let result = window
            .refresh(base.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(result.active_tree_changed && !result.conflict);
        window.model_mut().choose_pane(&second);
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
        assert!(!window.model().focus_mode()); // A replacement tab starts unexpanded.
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
}
