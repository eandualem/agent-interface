//! Binding a workspace saved without a profile to a profile, keeping its tabs
//! and panes: the app's Choose profile.

use std::path::{Path, PathBuf};

use agent_interface_core::{Leaf, Node, Workspace};
use agent_interface_roster::profiles::Connection;

use crate::library::{self, location::resolve_path};

/// What binding a workspace without a profile to one profile needs: that
/// profile and its tmux server, and the tmux server the workspace used unbound.
pub struct Binding {
    profile_id: String,
    /// The profile's tmux server, resolved; none when it cannot be.
    profile_server: Option<PathBuf>,
    source: String,
    resolved_source: Option<PathBuf>,
}

impl Binding {
    /// Bind to the profile `connection` reaches. `source` is the tmux server
    /// the workspace used unbound. This reads the file system.
    pub fn new(connection: &Connection, source: &Path) -> Result<Self, String> {
        let resolved_source = resolve_path(source).ok();
        let source = source
            .to_str()
            .ok_or("The tmux socket path is not UTF-8.")?
            .to_owned();
        Ok(Self {
            profile_id: connection.identity().profile_id.clone(),
            profile_server: resolve_path(connection.tmux_socket()).ok(),
            source,
            resolved_source,
        })
    }

    /// Bind `space`, saved without a profile, to the profile, in the library
    /// at `directory`. Every attached session stays on the server it is on:
    /// one already on the profile's server, under any spelling of its path,
    /// follows the profile.
    /// Ordinary shell panes become shells of the profile, as in any bound
    /// workspace, so the tmux server of each shell they showed is returned: it
    /// keeps running. An error means a shell could not be looked for, and
    /// `space` must not be saved.
    pub fn apply(&self, space: &mut Workspace, directory: &Path) -> Result<Vec<PathBuf>, String> {
        let mut shells = Vec::new();
        let mut unread = None;
        for tab in &mut space.tabs {
            each_leaf(&mut tab.tree, &mut |leaf| {
                // A session without a socket means the unbound server while
                // unbound and the profile's server afterwards, so it is pinned
                // to the unbound server, where it still is, unless that is the
                // profile's own server.
                if let Some(attachment) = leaf
                    .attachment
                    .as_mut()
                    .filter(|a| a.remote_profile.is_none())
                {
                    let server = match &attachment.source_socket {
                        Some(socket) => resolve_path(Path::new(socket)).ok(),
                        None => self.resolved_source.clone(),
                    };
                    if self.profile_server.is_some() && server == self.profile_server {
                        attachment.source_socket = None;
                    } else if attachment.source_socket.is_none() {
                        attachment.source_socket = Some(self.source.clone());
                    }
                }
                // A shell also waits behind an attachment, a chooser or a view.
                match library::existing_shell_socket(directory, &leaf.id) {
                    Ok(Some(socket)) => {
                        // The profile's shell starts where this one is now.
                        let _ = library::remember_shell_directory(leaf, &socket);
                        shells.push(socket);
                    }
                    Ok(None) => {}
                    // A shell that cannot be looked for would be left
                    // running unreported.
                    Err(error) => unread = Some(error.to_string()),
                }
                // A profile refuses to start a shell in a folder that is gone,
                // where an unbound shell would have started in the home folder.
                if leaf
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| !Path::new(cwd).is_dir())
                {
                    leaf.cwd = None;
                }
            });
        }
        if let Some(error) = unread {
            return Err(format!("The library's shells could not be read: {error}"));
        }
        space.profile_id = Some(self.profile_id.clone());
        Ok(shells)
    }
}

fn each_leaf(node: &mut Node, visit: &mut dyn FnMut(&mut Leaf)) {
    match node {
        Node::Leaf(leaf) => visit(leaf),
        Node::Split { first, second, .. } => {
            each_leaf(first, visit);
            each_leaf(second, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_core::Attachment;

    #[test]
    fn a_workspace_without_a_profile_is_bound_with_its_panes() {
        let mut layout = agent_interface_core::Layout::initial("/gone/folder".into());
        layout.attach(Some(Attachment {
            remote_profile: None,
            session: "worker".into(),
            source_socket: None,
        }));
        assert!(layout.split(agent_interface_core::Direction::Right, "/tmp".into(), true));
        layout.attach(Some(Attachment {
            remote_profile: None,
            session: "notes".into(),
            source_socket: Some("/tmp/other".into()),
        }));
        let binding = Binding {
            profile_id: "main".into(),
            profile_server: None,
            source: "/tmp/default".into(),
            resolved_source: None,
        };
        let library = tempfile::tempdir().unwrap();
        let space = &mut layout.workspaces[0];
        let shells = binding.apply(space, library.path()).unwrap();
        assert!(shells.is_empty());
        assert_eq!(space.profile_id.as_deref(), Some("main"));
        let leaves = space.tabs[0].tree.leaves();
        let sockets: Vec<_> = leaves
            .iter()
            .map(|leaf| leaf.attachment.as_ref().unwrap().source_socket.as_deref())
            .collect();
        // Each session keeps its server.
        assert_eq!(sockets, [Some("/tmp/default"), Some("/tmp/other")]);
        // A folder that is gone is not kept for the profile's shell.
        assert_eq!(leaves[0].cwd, None);
    }

    #[test]
    fn a_session_on_the_profiles_own_server_follows_it_whatever_the_spelling() {
        let folder = tempfile::tempdir().unwrap();
        let server = folder.path().join("profile.sock");
        std::fs::write(&server, b"").unwrap();
        let alias = folder.path().join("alias.sock");
        std::os::unix::fs::symlink(&server, &alias).unwrap();
        let mut layout = agent_interface_core::Layout::initial("/tmp".into());
        layout.attach(Some(Attachment {
            remote_profile: None,
            session: "worker".into(),
            source_socket: Some(alias.to_str().unwrap().into()),
        }));
        assert!(layout.split(agent_interface_core::Direction::Right, "/tmp".into(), true));
        layout.attach(Some(Attachment {
            remote_profile: None,
            session: "notes".into(),
            source_socket: None,
        }));
        let binding = Binding {
            profile_id: "main".into(),
            profile_server: resolve_path(&server).ok(),
            source: "/tmp/default".into(),
            resolved_source: None,
        };
        let library = tempfile::tempdir().unwrap();
        let space = &mut layout.workspaces[0];
        binding.apply(space, library.path()).unwrap();
        let sockets: Vec<_> = space.tabs[0]
            .tree
            .leaves()
            .iter()
            .map(|leaf| leaf.attachment.as_ref().unwrap().source_socket.clone())
            .collect();
        assert_eq!(sockets, [None, Some("/tmp/default".into())]);
    }
}
