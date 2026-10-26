//! Detached workspace choices for the native chooser.

use crate::library::WindowLayout;
use agent_interface_core::{Layout, RenameOutcome};
use agent_interface_store::Document;

pub(super) fn create(window: &WindowLayout, name: &str) -> Option<(Document, Layout)> {
    let (base, mut proposed) = window.save_snapshot();
    proposed.add_workspace(String::new());
    // Use the same printable-name normalization as native renaming.
    let edit = proposed.edit_workspace_name()?;
    (edit.apply(&mut proposed, name) == RenameOutcome::Applied).then_some((base, proposed))
}

pub(super) fn delete(window: &WindowLayout, expected: &str) -> Option<(Document, Layout)> {
    let (base, mut proposed) = window.save_snapshot();
    (proposed.selected == expected && proposed.delete_workspace()).then_some((base, proposed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn window() -> WindowLayout {
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":true}"#;
        WindowLayout::new(Document::parse(raw.into(), Path::new("layouts.db")).unwrap())
    }

    #[test]
    fn creation_normalizes_the_name_and_stays_detached_until_saved() {
        let window = window();
        assert!(create(&window, "  ").is_none());
        let (_, proposed) = create(&window, "  Workspace\n  ").unwrap();
        assert_eq!(window.model().workspaces.len(), 1);
        assert_eq!(proposed.workspaces.len(), 2);
        let added = proposed.workspace().unwrap();
        assert_eq!(added.name, "Workspace");
        assert!(added.tabs.is_empty());
        assert!(added.selected.is_empty());
        assert_eq!(added.id.len(), 12);
        assert_ne!(added.id, window.model().selected);
        assert!(!proposed.focus_mode());
        let (_, bounded) = create(&window, &"x".repeat(81)).unwrap();
        assert_eq!(bounded.workspace().unwrap().name.len(), 80);
        let mut displayed = window.model().clone();
        super::super::layout_edit::select_created(&mut displayed, &proposed);
        // The committed model supplies the new workspace before following it.
        assert_eq!(displayed.selected, window.model().selected);
        displayed = proposed.clone();
        displayed.choose_workspace(&window.model().selected);
        super::super::layout_edit::select_created(&mut displayed, &proposed);
        assert_eq!(displayed.selected, proposed.selected);
        assert!(!displayed.focus_mode());
    }

    #[test]
    fn deletion_rejects_tabs_last_workspace_and_stale_selection() {
        let mut window = window();
        let first = window.model().selected.clone();
        assert!(delete(&window, &first).is_none());
        window.model_mut().add_workspace("Other".into());
        let other = window.model().selected.clone();
        assert!(delete(&window, &first).is_none());
        let (_, deleted) = delete(&window, &other).unwrap();
        assert_eq!(window.model().workspaces.len(), 2);
        assert_eq!(deleted.workspaces.len(), 1);
        assert_eq!(deleted.selected, first);
        assert!(!deleted.focus_mode());
        window.model_mut().add_tab(None, "/tmp".into(), true);
        assert!(delete(&window, &other).is_none());
    }
}
