//! Binding a workspace saved without a profile to a profile, keeping its tabs
//! and panes: the app's Choose profile, and the one-off migration for a
//! library that was used with the Python Backbone service, which opens no
//! window.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use agent_interface_core::{Attachment, Leaf, Node, View, ViewKind, Workspace};
use agent_interface_roster::{profiles::Connection, views};
use agent_interface_store::Store;

use crate::library::{self, location::resolve_path};

/// What binding a workspace without a profile to one profile needs: that
/// profile's agents and the tmux server the workspace used unbound.
pub struct Binding {
    profile_id: String,
    /// Its agents, each with the registration that a bound agent view keeps.
    agents: HashMap<String, Option<String>>,
    source: String,
    resolved: Option<PathBuf>,
}

impl Binding {
    /// Read the profile's agents through `connection`. `source` is the tmux
    /// server the workspace used unbound. This reads from the service and the
    /// file system.
    pub fn read(connection: &Connection, source: &Path) -> Result<Self, String> {
        let live = || true;
        let team = views::bound::read(connection, &views::bound::Request::Team, &live);
        let Ok(views::Data::Team(team)) = team.result else {
            return Err("The profile's agents could not be read.".into());
        };
        let agents = team
            .members
            .into_iter()
            .map(|member| (member.name, member.registration_id))
            .collect();
        // A session that is not an agent is pinned to the unbound tmux server
        // by its path, so that path must be known.
        let resolved = resolve_path(source).ok();
        let source = source
            .to_str()
            .ok_or("The tmux socket path is not UTF-8.")?
            .to_owned();
        Ok(Self {
            profile_id: connection.identity().profile_id.clone(),
            agents,
            source,
            resolved,
        })
    }

    /// The unbound tmux server, where every session that is not one of the
    /// profile's agents is left.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Bind `space`, saved without a profile, to the profile, in the library
    /// at `directory`. The profile's agents' attachments follow the profile;
    /// any other session stays where it is. Ordinary shell panes become shells
    /// of the profile, as in any bound workspace, so each shell they showed is
    /// returned with the command that reaches it: it keeps running. An error
    /// means a shell could not be looked for, and `space` must not be saved.
    pub fn apply(&self, space: &mut Workspace, directory: &Path) -> Result<Vec<String>, String> {
        let names_source = |socket: &str| {
            socket == self.source
                || (self.resolved.is_some()
                    && resolve_path(Path::new(socket)).ok() == self.resolved)
        };
        let mut report = Vec::new();
        let mut unread = None;
        for tab in &mut space.tabs {
            let (workspace, name) = (&space.name, &tab.name);
            each_leaf(&mut tab.tree, &mut |leaf| {
                if let Some(attachment) = &mut leaf.attachment {
                    let agent = self.agents.contains_key(&attachment.session);
                    migrate_attachment(attachment, agent, &self.source, &names_source);
                }
                if let Some(view) = &mut leaf.view
                    && !register_agent_view(view, &self.agents)
                {
                    report.push(format!(
                        "The view of agent {}, in {workspace} > {name}, is not of this profile: reopen it from the Team view",
                        view.agent.as_deref().unwrap_or_default(),
                    ));
                }
                // A shell also waits behind an attachment, a chooser or a view.
                match library::existing_shell_socket(directory, &leaf.id) {
                    Ok(Some(socket)) => {
                        // The profile's shell starts where this one is now.
                        let _ = library::remember_shell_directory(leaf, &socket);
                        let session = format!("terminal-{}", leaf.id);
                        let command = agent_interface_tmux::discovery::read_session_command(
                            &socket, &session,
                        );
                        let command = match command.as_deref() {
                            None => "running an unknown command",
                            // Its command ended and the pane was kept.
                            Some("") => "its command has ended",
                            Some(command) => &format!("running {command}"),
                        };
                        report.push(format!(
                            "Shell left on its server, from {workspace} > {name}, {command}: tmux -S {} attach -t {session}",
                            socket.display(),
                        ));
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
        Ok(report)
    }
}

/// Bind every workspace without a profile to the one profile that the Rust
/// service serves with its home on this computer, and save the library.
///
/// The Python service served such workspaces without a binding, and its agents
/// ran on the unbound tmux server. Each workspace is bound as
/// [`Binding::apply`] does. Returns what was done, one line each. Nothing is
/// written when anything needed cannot be read. The save follows the store's
/// rules: a compatible change made meanwhile is merged, and a conflicting one
/// stops the command without writing.
pub fn bind_home_profile(directory: Option<&Path>) -> Result<Vec<String>, String> {
    let live = || true;
    let client = crate::desktop::profile_client().ok_or("The backbone command was not found.")?;
    let home = client
        .home(&live)
        .map_err(|_| "The profile list could not be read.")?
        .ok_or("The service does not list exactly one profile with its home on this computer.")?;
    let connection = client
        .show(&home.profile_id, &live)
        .ok()
        .and_then(|profile| profile.connect().ok())
        .ok_or("The profile did not answer.")?;
    // The earlier service kept its agents on the default server, wherever
    // this command is run from.
    let source = library::location::default_source_socket().map_err(|error| error.to_string())?;
    let binding = Binding::read(&connection, &source)?;

    let directory = library::library_directory(directory).map_err(|error| error.to_string())?;
    let mut store = Store::open_existing_writable(&directory).map_err(|error| error.to_string())?;
    let mut base = store.load().map_err(|error| error.to_string())?;
    let mut layout = base.model().clone();
    let mut report = Vec::new();
    for space in &mut layout.workspaces {
        if space.profile_id.is_some() {
            continue;
        }
        report.extend(binding.apply(space, &directory)?);
        report.push(format!(
            "Bound workspace {} ({}) to profile {} ({})",
            space.name, space.id, home.display_name, home.profile_id
        ));
    }
    if report.is_empty() {
        return Ok(vec!["Every workspace already has a profile.".into()]);
    }
    // The default server follows TMUX_TMPDIR, as tmux does, so say which.
    report.push(format!(
        "Panes of the profile's agents now follow the profile's tmux server. The earlier server, where every other session was left, is {}",
        binding.source()
    ));
    let saved = store
        .save(&mut base, &layout)
        .map_err(|error| error.to_string())?;
    if saved.conflict {
        return Err(
            "A change made to the library meanwhile conflicts with this one, so nothing was saved. Run this again."
                .into(),
        );
    }
    Ok(report)
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

/// Where an attachment on the unbound tmux server belongs once its workspace
/// is bound. An attachment without a socket means that server while unbound
/// and the profile's server afterwards. So one of the profile's agents loses
/// its socket and follows the profile, where the agent now runs, and any other
/// session is pinned to the unbound server, where it still is: its bound pane
/// then says that the session is on another server.
fn migrate_attachment(
    attachment: &mut Attachment,
    agent: bool,
    source: &str,
    names_source: &dyn Fn(&str) -> bool,
) {
    match &attachment.source_socket {
        Some(socket) if agent && names_source(socket) => attachment.source_socket = None,
        None if !agent => attachment.source_socket = Some(source.to_owned()),
        _ => {}
    }
}

/// Gives a saved agent view the registration of its agent, which a bound
/// agent view needs to show anything. False when the profile has no such
/// agent, so the view stays as it was and must be opened again.
fn register_agent_view(view: &mut View, agents: &HashMap<String, Option<String>>) -> bool {
    if view.kind != ViewKind::Agent || view.registration_id.is_some() {
        return true;
    }
    let registration = view.agent.as_ref().and_then(|agent| agents.get(agent));
    view.registration_id = registration.cloned().flatten();
    // A registration the library would refuse is not kept.
    if !view.is_valid() {
        view.registration_id = None;
    }
    view.registration_id.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_attachments_to_the_profiles_agents_follow_it() {
        let migrated = |socket: Option<&str>, agent: bool| {
            let mut attachment = Attachment {
                session: "worker".into(),
                source_socket: socket.map(str::to_owned),
            };
            migrate_attachment(&mut attachment, agent, "/tmp/default", &|socket| {
                socket == "/tmp/default"
            });
            attachment.source_socket
        };
        // An agent on the unbound server follows the profile.
        assert_eq!(migrated(Some("/tmp/default"), true), None);
        assert_eq!(migrated(None, true), None);
        // Another session stays on the unbound server, named or not.
        for socket in [Some("/tmp/default"), None] {
            assert_eq!(migrated(socket, false).as_deref(), Some("/tmp/default"));
        }
        // A session on a third server is left alone.
        assert_eq!(
            migrated(Some("/tmp/other"), true).as_deref(),
            Some("/tmp/other")
        );
    }

    #[test]
    fn a_saved_agent_view_takes_its_agents_registration() {
        let registration = "0123456789abcdef0123456789abcdef";
        let agents = HashMap::from([
            ("worker".to_owned(), Some(registration.to_owned())),
            ("unregistered".to_owned(), None),
            ("malformed".to_owned(), Some(registration.to_uppercase())),
        ]);
        let mut view = View::agent("worker".into(), None);
        assert!(register_agent_view(&mut view, &agents));
        assert_eq!(view.registration_id.as_deref(), Some(registration));
        assert!(view.is_valid());
        // An agent the profile does not have, has not registered, or names
        // with a registration that a library does not hold.
        for name in ["stranger", "unregistered", "malformed"] {
            let mut view = View::agent(name.into(), None);
            assert!(!register_agent_view(&mut view, &agents));
            assert_eq!(view.registration_id, None);
        }
        // Any other view needs none.
        assert!(register_agent_view(&mut View::of(ViewKind::Team), &agents));
    }

    #[test]
    fn a_workspace_without_a_profile_is_bound_with_its_panes() {
        let mut layout = agent_interface_core::Layout::initial("/gone/folder".into());
        layout.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: None,
        }));
        assert!(layout.split(agent_interface_core::Direction::Right, "/tmp".into(), true));
        layout.attach(Some(Attachment {
            session: "notes".into(),
            source_socket: None,
        }));
        let binding = Binding {
            profile_id: "main".into(),
            agents: HashMap::from([("worker".to_owned(), None)]),
            source: "/tmp/default".into(),
            resolved: None,
        };
        let library = tempfile::tempdir().unwrap();
        let space = &mut layout.workspaces[0];
        assert!(binding.apply(space, library.path()).unwrap().is_empty());
        assert_eq!(space.profile_id.as_deref(), Some("main"));
        let leaves = space.tabs[0].tree.leaves();
        let sockets: Vec<_> = leaves
            .iter()
            .map(|leaf| leaf.attachment.as_ref().unwrap().source_socket.as_deref())
            .collect();
        // The agent follows the profile; the other session keeps its server.
        assert_eq!(sockets, [None, Some("/tmp/default")]);
        // A folder that is gone is not kept for the profile's shell.
        assert_eq!(leaves[0].cwd, None);
    }
}
