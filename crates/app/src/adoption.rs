//! A one-off migration for a library that was used with the Python Backbone
//! service: bind its workspaces that have no profile to the profile the Rust
//! service keeps on this computer. It opens no window.

use std::{collections::HashSet, path::Path};

use agent_interface_core::{Attachment, Leaf, Node};
use agent_interface_store::Store;

use crate::library::{self, location::resolve_path};

/// Bind every workspace without a profile to the one profile that the Rust
/// service serves with its home on this computer, and save the library.
///
/// The Python service served such workspaces without a binding, and its agents
/// ran on the unbound tmux server. Their attachments follow the profile; any
/// other session stays where it is. Ordinary shell panes become shells of the
/// profile, as in any bound workspace, so each shell they showed is listed
/// with the command that reaches it: it keeps running. Returns what was done,
/// one line each. Nothing is written when anything needed cannot be read. The
/// save follows the store's rules: a compatible change made meanwhile is
/// merged, and a conflicting one stops the command without writing.
pub fn bind_home_profile(directory: Option<&Path>) -> Result<Vec<String>, String> {
    let live = || true;
    let client = crate::desktop::profile_client().ok_or("The backbone command was not found.")?;
    let home = client
        .home(&live)
        .map_err(|_| "The profile list could not be read.")?
        .ok_or("The service does not list exactly one profile with its home on this computer.")?;
    let roster = client
        .show(&home.profile_id, &live)
        .ok()
        .and_then(|profile| profile.connect().ok()?.roster().ok())
        .ok_or("The profile's agents could not be read.")?;
    let agents: HashSet<String> = roster.states.into_keys().collect();
    // A session that is not an agent is pinned to the unbound tmux server by
    // its path, so that path must be known. The earlier service kept its
    // agents on the default server, wherever this command is run from.
    let source = library::location::default_source_socket().map_err(|error| error.to_string())?;
    let source_text = source
        .to_str()
        .ok_or("The tmux socket path is not UTF-8.")?;
    let resolved = resolve_path(&source).ok();
    let names_source = |socket: &str| {
        socket == source_text
            || (resolved.is_some() && resolve_path(Path::new(socket)).ok() == resolved)
    };

    let directory = library::library_directory(directory).map_err(|error| error.to_string())?;
    let mut store = Store::open_existing_writable(&directory).map_err(|error| error.to_string())?;
    let mut base = store.load().map_err(|error| error.to_string())?;
    let mut layout = base.model().clone();
    let mut report = Vec::new();
    let mut unread = None;
    for space in &mut layout.workspaces {
        if space.profile_id.is_some() {
            continue;
        }
        for tab in &mut space.tabs {
            let (workspace, name) = (&space.name, &tab.name);
            each_leaf(&mut tab.tree, &mut |leaf| {
                if let Some(attachment) = &mut leaf.attachment {
                    let agent = agents.contains(&attachment.session);
                    migrate_attachment(attachment, agent, source_text, &names_source);
                }
                // A shell also waits behind an attachment, a chooser or a view.
                match library::existing_shell_socket(&directory, &leaf.id) {
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
                    // running unreported, with no second run to find it.
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
        space.profile_id = Some(home.profile_id.clone());
        report.push(format!(
            "Bound workspace {} ({}) to profile {} ({})",
            space.name, space.id, home.display_name, home.profile_id
        ));
    }
    if let Some(error) = unread {
        return Err(format!("The library's shells could not be read: {error}"));
    }
    if report.is_empty() {
        return Ok(vec!["Every workspace already has a profile.".into()]);
    }
    // The default server follows TMUX_TMPDIR, as tmux does, so say which.
    report.push(format!(
        "Agents' sessions were taken to be on the tmux server at {source_text}"
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
}
