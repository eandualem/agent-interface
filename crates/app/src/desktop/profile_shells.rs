//! The shells a profile opens for its workspace's panes end with their panes.
//!
//! A pane in a profile's workspace with nothing attached runs the shell the
//! profile opened for it, `shell-<pane id>` on the profile's tmux server
//! (`POST /api/profile/shells`). Once the pane is gone from the saved layout
//! the app asks the service to end it (`DELETE /api/profile/shells/{id}`);
//! the app never kills a session itself.

use std::{path::Path, process::Command};

use agent_interface_core::{Layout, Leaf};

/// The IDs of `leaves` whose terminal is their profile's own shell: those
/// that run a terminal with nothing attached.
pub(super) fn own<'a>(leaves: impl IntoIterator<Item = &'a Leaf>) -> Vec<String> {
    leaves
        .into_iter()
        .filter(|leaf| leaf.runs_terminal() && leaf.attachment.is_none())
        .map(|leaf| leaf.id.clone())
        .collect()
}

/// Those of `ids` no pane in `layout` holds: a pane closed here, not one
/// that moved to another tab.
pub(super) fn released(layout: &Layout, ids: Vec<String>) -> Vec<String> {
    let held = |id: &String| {
        layout.workspaces.iter().any(|space| {
            space
                .tabs
                .iter()
                .any(|tab| tab.tree.leaves().iter().any(|leaf| &leaf.id == id))
        })
    };
    ids.into_iter().filter(|id| !held(id)).collect()
}

/// Whether a command runs in the foreground of one of the shells `ids` on
/// the tmux server at `socket`, so ending it could lose work. A shell that
/// is not running has nothing to lose; one whose processes cannot be read
/// counts as running. Runs on an I/O worker.
pub(super) fn running(socket: &Path, ids: &[String]) -> bool {
    let Ok(tmux) = agent_interface_tmux::connection::tmux_binary() else {
        return true;
    };
    ids.iter().any(|id| {
        let listed = Command::new(&tmux)
            .arg("-S")
            .arg(socket)
            .args(["list-panes", "-s", "-t", &format!("=shell-{id}"), "-F"])
            .arg("#{pane_pid}")
            .output();
        match listed {
            Ok(listed) if listed.status.success() => String::from_utf8_lossy(&listed.stdout)
                .split_whitespace()
                .any(|pid| !shell_in_foreground(pid)),
            // The session has ended or the server is gone.
            _ => false,
        }
    })
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

    fn leaf(id: &str) -> Leaf {
        Leaf {
            id: id.into(),
            attachment: None,
            cwd: None,
            empty: false,
            view: None,
        }
    }

    #[test]
    fn only_a_terminal_with_nothing_attached_is_the_profile_s_own_shell() {
        let shell = leaf("0123456789ab");
        let mut attached = leaf("123456789abc");
        attached.attachment = Some(agent_interface_core::Attachment {
            session: "worker".into(),
            remote_profile: None,
            source_socket: None,
        });
        let mut empty = leaf("23456789abcd");
        empty.empty = true;
        assert_eq!(own([&shell, &attached, &empty]), ["0123456789ab"]);
    }

    #[test]
    fn only_a_shell_no_pane_holds_any_longer_is_released() {
        let mut model = Layout::initial("/".into());
        model.add_tab(None, "/".into(), true);
        // A pane in another tab, or one moved there, still holds its shell.
        let kept = model.workspaces[0].tabs[0].tree.leaves()[0].id.clone();
        let closed = "0123456789ab".to_owned();
        assert_eq!(released(&model, vec![kept, closed.clone()]), [closed]);
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
        assert!(running(socket, &[idle.clone(), busy]));
        // A shell that has ended has nothing to lose.
        assert!(!running(socket, &["23456789abcd".to_owned()]));
    }
}
