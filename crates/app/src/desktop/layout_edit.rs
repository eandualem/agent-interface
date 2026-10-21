//! Detached structural edits and ordinary-shell snapshots for native controls.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use tmux_workspaces_core::{Direction, Layout, Node, Tab};

use crate::library;
use tmux_workspaces_store::{Document, Error, Reconciliation, Store};

#[derive(Clone, Copy)]
pub(super) enum Create {
    Tab,
    Split(Direction),
}

impl Create {
    pub fn apply(self, model: &mut Layout, launch_directory: &str) -> bool {
        match self {
            Self::Tab => model.add_tab(None, launch_directory.into(), true),
            Self::Split(direction) => {
                let cwd = model
                    .pane()
                    .and_then(|pane| pane.cwd.as_deref())
                    .filter(|cwd| !cwd.is_empty())
                    .unwrap_or(launch_directory)
                    .to_owned();
                model.split(direction, cwd, true)
            }
        }
    }
}

/// Run on the save worker, including on retry. Creation is consumed exactly once;
/// even an opening failure leaves its generated proposal available for retry.
pub(super) fn save(
    directory: Option<&Path>,
    base: &mut Document,
    local: &mut Layout,
    previous: Option<&Tab>,
    creation: Option<(Create, String)>,
) -> Result<Reconciliation, Error> {
    let directory = library::library_directory(directory);
    if let Ok(directory) = &directory
        && let Some(previous) = previous
        && let Ok(Some(socket)) = library::existing_shell_socket(directory)
    {
        remember_directories(local, previous, &socket);
    }
    if let Some((action, cwd)) = creation {
        action.apply(local, &cwd);
    }
    directory
        .and_then(|directory| Store::open_existing_writable(&directory))
        .and_then(|mut store| store.save(base, local))
}

/// The visible selection before creation, used to respect navigation during save.
#[derive(PartialEq)]
pub(super) struct Selection {
    workspace: String,
    tab: Option<String>,
    pane: Option<String>,
    focus: bool,
}

impl Selection {
    pub fn capture(model: &Layout) -> Self {
        Self {
            workspace: model.selected.clone(),
            tab: model.tab().map(|tab| tab.id.clone()),
            pane: model.pane().map(|pane| pane.id.clone()),
            focus: model.focus,
        }
    }

    pub fn unchanged(&self, model: &Layout) -> bool {
        *self == Self::capture(model)
    }
}

pub(super) fn select_created(model: &mut Layout, created: &Layout) {
    // Do not reset remembered selections in other workspaces or tabs.
    if let (Some(tab), Some(pane)) = (created.tab(), created.pane())
        && model.choose_workspace(&created.selected)
        && model.choose_tab(&tab.id)
        && model.choose_pane(&pane.id)
    {
        model.focus = created.focus;
    }
}

/// Refresh only previously nonempty leaves from their own ordinary shells.
/// Reads run before creation on the save worker. Failures retain saved paths.
pub(super) fn remember_directories(model: &mut Layout, previous: &Tab, socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut directories = HashMap::new();
    for leaf in previous
        .tree
        .leaves()
        .into_iter()
        .filter(|leaf| !leaf.empty)
    {
        if Instant::now() >= deadline {
            break;
        }
        let mut observed = leaf.clone();
        if library::remember_shell_directory(&mut observed, socket).is_ok() {
            directories.insert(observed.id, observed.cwd);
        }
    }
    fn apply(node: &mut Node, directories: &HashMap<String, Option<String>>) {
        match node {
            Node::Leaf(leaf) => {
                if let Some(cwd) = directories.get(&leaf.id) {
                    leaf.cwd.clone_from(cwd);
                }
            }
            Node::Split { first, second, .. } => {
                apply(first, directories);
                apply(second, directories);
            }
        }
    }
    for workspace in &mut model.workspaces {
        if let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == previous.id) {
            apply(&mut tab.tree, &directories);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::WindowLayout;
    use tmux_workspaces_core::Attachment;

    #[test]
    fn creation_waits_for_commit_retries_the_same_ids_and_respects_navigation() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        let initial = Document::parse(raw.into(), &db).unwrap();
        let mut model = initial.model().clone();
        model.add_tab(Some("First".into()), "/saved".into(), false);
        model.add_tab(Some("Second".into()), "/saved".into(), true);
        let second = model.tab().unwrap().id.clone();
        let first = model.workspace().unwrap().tabs[0].id.clone();
        model.choose_tab(&first);
        let initial = initial.with_model(&model, &db).unwrap();
        let mut window = WindowLayout::new(initial.clone());
        let original = Selection::capture(window.model());
        let (mut base, mut local) = window.save_snapshot();
        let previous = local.tab().cloned();
        // A missing store fails after producing the complete detached candidate.
        assert!(
            save(
                Some(directory.path()),
                &mut base,
                &mut local,
                previous.as_ref(),
                Some((Create::Tab, "/launch".into()))
            )
            .is_err()
        );
        let created = local.tab().unwrap().clone();
        assert_eq!(created.tree.leaves()[0].cwd.as_deref(), Some("/launch"));
        assert!(created.tree.leaves()[0].empty);
        assert_eq!(window.model().workspace().unwrap().tabs.len(), 2);
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            .unwrap();
        connection
            .execute(
                "INSERT INTO terminal_layout VALUES (1, ?1)",
                [initial.raw_json()],
            )
            .unwrap();
        let result = save(
            Some(directory.path()),
            &mut base,
            &mut local,
            previous.as_ref(),
            None,
        )
        .unwrap();
        assert_eq!(
            local.tab().unwrap(),
            &created,
            "retry does not allocate another tab"
        );
        window.model_mut().choose_tab(&second);
        assert!(!original.unchanged(window.model()));
        window.saved(base, &result);
        assert_eq!(window.model().tab().unwrap().id, second);
        assert_eq!(window.model().workspace().unwrap().tabs.len(), 3);

        window.model_mut().choose_tab(&first);
        window.model_mut().focus = true;
        let original = Selection::capture(window.model());
        let (mut base, mut local) = window.save_snapshot();
        let previous = local.tab().cloned();
        let result = save(
            Some(directory.path()),
            &mut base,
            &mut local,
            previous.as_ref(),
            Some((Create::Split(Direction::Right), "/launch".into())),
        )
        .unwrap();
        assert!(original.unchanged(window.model()));
        assert_eq!(window.model().tab().unwrap().tree.leaves().len(), 1);
        window.saved(base, &result);
        select_created(window.model_mut(), &local);
        assert!(!window.model().focus);
        let leaves = window.model().tab().unwrap().tree.leaves();
        assert_eq!(leaves[0], previous.as_ref().unwrap().tree.leaves()[0]);
        assert!(leaves[1].empty);
        assert_eq!(leaves[1].cwd.as_deref(), Some("/saved"));
        assert_eq!(window.model().pane(), Some(leaves[1]));
    }

    #[test]
    fn snapshots_use_only_nonempty_ordinary_sessions_in_the_previous_tab() {
        use tmux_workspaces_test_support::TmuxServer;
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let directory = server.address().0.join("current directory");
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let mut model = Layout::initial("/saved".into());
        model.attach(Some(Attachment {
            session: "external".into(),
            source_socket: Some("/external/socket".into()),
        }));
        model.split(Direction::Right, "/saved".into(), false);
        model.split(Direction::Below, "/saved".into(), true);
        let previous = model.tab().unwrap().clone();
        // Even an existing orphan session for the empty leaf must not be read.
        for leaf in previous.tree.leaves() {
            server
                .run(&[
                    "new-session",
                    "-d",
                    "-s",
                    &library::ordinary_session(&leaf.id).unwrap(),
                    "-c",
                    directory.to_str().unwrap(),
                    "sleep 60",
                ])
                .unwrap();
        }
        remember_directories(&mut model, &previous, Path::new(socket.trim_end()));
        let leaves = model.tab().unwrap().tree.leaves();
        assert_eq!(leaves[0].cwd.as_deref(), directory.to_str());
        assert_eq!(leaves[1].cwd.as_deref(), directory.to_str());
        assert_eq!(leaves[2].cwd.as_deref(), Some("/saved"));
        assert_eq!(leaves[0].attachment, previous.tree.leaves()[0].attachment);
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
    }
}
