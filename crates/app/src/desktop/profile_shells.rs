//! The shells a profile opens for its workspace's panes end with their panes.
//!
//! A pane in a profile's workspace with nothing attached runs the shell the
//! profile opened for it, `shell-<pane id>` on the profile's tmux server
//! (`POST /api/profile/shells`). A pane detached and given another session
//! or a view keeps its ID, and its shell runs on, so any closed pane may hold
//! one. Once the pane is gone from the saved layout the app asks the service
//! to end it (`DELETE /api/profile/shells/{id}`, also answered when there is
//! none); the app never kills a session itself.

use std::{path::Path, process::Command};

use agent_interface_core::Layout;

/// Those of `ids` no pane in `layout` holds: a pane closed here, not one
/// that moved to another tab, and not a shell another pane is attached to.
pub(super) fn released(layout: &Layout, ids: Vec<String>) -> Vec<String> {
    let held = |id: &String| {
        let session = format!("shell-{id}");
        layout.workspaces.iter().any(|space| {
            space.tabs.iter().any(|tab| {
                tab.tree.leaves().iter().any(|leaf| {
                    &leaf.id == id
                        || leaf
                            .attachment
                            .as_ref()
                            .is_some_and(|a| a.session == session)
                })
            })
        })
    };
    ids.into_iter().filter(|id| !held(id)).collect()
}

/// Whether a command runs in the foreground of one of the shells `ids` on
/// the tmux server at `socket`, so ending it could lose work. A shell tmux
/// says is not there has nothing to lose; one that cannot be read counts as
/// running. Runs on an I/O worker.
pub(super) fn running(socket: &Path, ids: &[String]) -> bool {
    let Ok(tmux) = agent_interface_tmux::connection::tmux_binary() else {
        return true;
    };
    ids.iter().any(|id| {
        let listed = Command::new(&tmux)
            .arg("-S")
            .arg(socket)
            .args(["list-panes", "-s", "-t", &format!("=shell-{id}:"), "-F"])
            .arg("#{pane_pid}")
            .output();
        match listed {
            Ok(listed) if listed.status.success() => String::from_utf8_lossy(&listed.stdout)
                .split_whitespace()
                .any(|pid| !shell_in_foreground(pid)),
            Ok(listed) => !absent(&String::from_utf8_lossy(&listed.stderr)),
            Err(_) => true,
        }
    })
}

/// Whether tmux's `error` says the session or its server is not there.
fn absent(error: &str) -> bool {
    [
        "can't find session",
        "no server running",
        "No such file or directory",
    ]
    .iter()
    .any(|absent| error.contains(absent))
}

/// Whether the terminal of the process `pid` has that process's own group
/// in the foreground: the shell waits at its prompt.
fn shell_in_foreground(pid: &str) -> bool {
    let Ok(read) = Command::new("ps")
        .args(["-o", "tpgid=", "-p", pid])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&read.stdout).trim() == pid
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_test_support::TmuxServer;
    use std::time::{Duration, Instant};

    #[test]
    fn only_a_shell_no_pane_holds_any_longer_is_released() {
        let mut model = Layout::initial("/".into());
        model.add_tab(None, "/".into(), true);
        // A pane in another tab, or one moved there, still holds its shell.
        let kept = model.workspaces[0].tabs[0].tree.leaves()[0].id.clone();
        let closed = "0123456789ab".to_owned();
        assert_eq!(
            released(&model, vec![kept, closed.clone()]),
            std::slice::from_ref(&closed)
        );
        // A pane attached to the closed pane's shell still holds it.
        assert!(model.attach(Some(agent_interface_core::Attachment {
            session: format!("shell-{closed}"),
            remote_profile: None,
            source_socket: None,
        })));
        assert!(released(&model, vec![closed]).is_empty());
    }

    #[test]
    fn a_shell_at_its_prompt_is_not_running_and_one_with_a_command_is() {
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let socket = Path::new(socket.trim_end());
        let (idle, busy) = ("0123456789ab".to_owned(), "123456789abc".to_owned());
        for id in [&idle, &busy] {
            server
                .run(&["new-session", "-d", "-s", &format!("shell-{id}"), "sh"])
                .unwrap();
        }
        server
            .run(&[
                "send-keys",
                "-t",
                &format!("=shell-{busy}:"),
                "sleep 60",
                "Enter",
            ])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !running(socket, std::slice::from_ref(&busy)) {
            assert!(Instant::now() < deadline, "the command never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!running(socket, std::slice::from_ref(&idle)));
        assert!(running(socket, &[idle.clone(), busy.clone()]));
        // A shell that has ended, or a server that is gone, has nothing to lose.
        assert!(!running(socket, &["23456789abcd".to_owned()]));
        let gone = socket.with_file_name("gone");
        assert!(!running(&gone, std::slice::from_ref(&busy)));
        // Any other failure to read it may hide a running command.
        assert!(!absent("protocol version mismatch (client 8, server 9)"));
    }
}
