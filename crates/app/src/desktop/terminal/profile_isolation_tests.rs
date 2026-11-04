//! Exercise connection ownership through real, disposable terminal streams.

use super::*;
use crate::roster::content::tests::{Reply, provider_with_tmux};
use agent_interface_core::Attachment;
use agent_interface_test_support::TmuxServer;
use serde_json::json;
use std::time::{Duration, Instant};

fn take_text(ready: &Ready, forbidden: &str) -> String {
    let mut observed = String::new();
    if let Some(update) = ready.panes.as_ref().unwrap().take_update() {
        for (_, screen) in update.screens {
            let text: String = (0..screen.rows())
                .flat_map(|row| (0..screen.cols()).map(move |col| (row, col)))
                .filter_map(|(row, col)| screen.cell(row, col).map(|(text, _)| text))
                .collect();
            assert!(!text.contains(forbidden), "foreign output: {text}");
            observed.push_str(&text);
            observed.push('\n');
        }
    }
    observed
}

fn wait_for_text(ready: &Ready, expected: &str, forbidden: &str) {
    let panes = ready.panes.as_ref().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if take_text(ready, forbidden).contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "missing {expected}: {:?}",
            panes.failure()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn equal_terminal_ids_keep_streams_and_input_isolated_when_one_profile_stops() {
    let a = TmuxServer::start().unwrap();
    let b = TmuxServer::start().unwrap();
    let socket = |server: &TmuxServer| -> PathBuf {
        server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim()
            .into()
    };
    for (server, label) in [(&a, "A"), (&b, "B")] {
        // Identical session, window and pane IDs on separate private servers.
        server
            .run(&[
                "respawn-pane",
                "-k",
                "-t",
                "=fixture:",
                "sh",
                "-c",
                &format!("printf 'profile-{label}-ready\\n'; exec cat"),
            ])
            .unwrap();
    }
    assert_eq!(
        a.run(&[
            "display-message",
            "-p",
            "-t",
            "=fixture:",
            "#{session_id}/#{window_id}/#{pane_id}"
        ])
        .unwrap(),
        b.run(&[
            "display-message",
            "-p",
            "-t",
            "=fixture:",
            "#{session_id}/#{window_id}/#{pane_id}"
        ])
        .unwrap()
    );
    let make_target = |label: &str, own_socket: PathBuf, foreign_socket: PathBuf| {
        let identity = json!({"schema_version":1,"profile_id":format!("profile-{label}"),
            "installation_id":format!("installation-{label}"), "lifecycle_id":format!("lifecycle-{label}"),
            "baseline_version":"fixture", "tmux":{"socket_path":own_socket}});
        let (directory, connection, server) = provider_with_tmux(
            label,
            (0..2)
                .map(|_| Reply::ok("/api/profile", identity.clone()))
                .collect(),
            Some(own_socket),
        );
        (
            directory,
            server,
            Target {
                connection: Some(connection),
                leaf: Leaf {
                    id: "123456abcdef".into(),
                    cwd: None,
                    empty: false,
                    view: None,
                    attachment: Some(Attachment {
                        session: "fixture".into(),
                        source_socket: None,
                    }),
                },
                directory: None,
                // Even an available same-name legacy target must not receive I/O.
                source: Ok(foreign_socket),
            },
        )
    };
    let (_a_directory, a_provider, target_a) = make_target("a", socket(&a), socket(&b));
    let (_b_directory, b_provider, target_b) = make_target("b", socket(&b), socket(&a));
    assert!(!target_a.same_session(&target_b));
    let ready_a = connect(target_a).unwrap();
    let ready_b = connect(target_b).unwrap();
    a_provider.join().unwrap();
    b_provider.join().unwrap();
    assert_eq!(ready_a.input, ready_b.input);
    wait_for_text(&ready_a, "profile-A-ready", "profile-B-ready");
    wait_for_text(&ready_b, "profile-B-ready", "profile-A-ready");
    ready_a
        .panes
        .as_ref()
        .unwrap()
        .send_input(ready_a.input, b"only-A-input\r")
        .unwrap();
    ready_b
        .panes
        .as_ref()
        .unwrap()
        .send_input(ready_b.input, b"only-B-input\r")
        .unwrap();
    wait_for_text(&ready_a, "only-A-input", "only-B-input");
    wait_for_text(&ready_b, "only-B-input", "only-A-input");
    let capture = |server: &TmuxServer| {
        server
            .run(&["capture-pane", "-p", "-t", "=fixture:"])
            .unwrap()
    };
    assert!(!capture(&a).contains("only-B-input"));
    assert!(!capture(&b).contains("only-A-input"));

    // Stop only the test-owned A server while both native connections live.
    a.run(&["kill-server"]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready_a.panes.as_ref().unwrap().is_finished() {
        // Completion includes consumption of the worker's last screen update.
        // Input echo and cat output may arrive in separate updates.
        take_text(&ready_a, "only-B-input");
        assert!(
            Instant::now() < deadline,
            "stopped A connection stayed live"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        ready_a
            .panes
            .as_ref()
            .unwrap()
            .send_input(ready_a.input, b"must-not-fallback\r")
            .is_err()
    );
    assert!(!ready_b.panes.as_ref().unwrap().is_finished());
    ready_b
        .panes
        .as_ref()
        .unwrap()
        .send_input(ready_b.input, b"B-after-A-stop\r")
        .unwrap();
    wait_for_text(&ready_b, "B-after-A-stop", "must-not-fallback");
    let remaining = capture(&b);
    assert!(remaining.contains("B-after-A-stop"));
    assert!(!remaining.contains("must-not-fallback"));
    assert!(!remaining.contains("only-A-input"));
}

#[test]
fn bound_shell_attaches_to_the_provider_session_and_never_creates_one() {
    let server = TmuxServer::start().unwrap();
    let socket: PathBuf = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap()
        .trim()
        .into();
    server
        .run(&[
            "respawn-pane",
            "-k",
            "-t",
            "=fixture:",
            "sh",
            "-c",
            "printf 'provider-shell-ready\\n'; exec cat",
        ])
        .unwrap();
    let target = |replies: Vec<Reply>| {
        let (directory, connection, provider) =
            provider_with_tmux("a", replies, Some(socket.clone()));
        (
            directory,
            provider,
            Target {
                connection: Some(connection),
                leaf: Leaf {
                    id: "123456abcdef".into(),
                    cwd: None,
                    empty: false,
                    view: None,
                    attachment: None,
                },
                directory: None,
                source: Err("A bound shell must not use the legacy source.".into()),
            },
        )
    };
    let identity = json!({"schema_version":1,"profile_id":"profile-a",
        "installation_id":"installation-a","lifecycle_id":"lifecycle-a",
        "baseline_version":"fixture","tmux":{"socket_path":socket}});
    let shell = json!({"schema_version":1,"profile_id":"profile-a",
        "installation_id":"installation-a","lifecycle_id":"lifecycle-a",
        "shell_id":"123456abcdef","session":"fixture","created":true});
    let (_directory, provider, opened) = target(vec![
        Reply::ok("/api/profile", identity.clone()),
        Reply::post("/api/profile/shells", 200, shell),
        Reply::ok("/api/profile", identity.clone()),
    ]);
    let ready = connect(opened).unwrap();
    provider.join().unwrap();
    wait_for_text(&ready, "provider-shell-ready", "foreign");
    let sessions = server.run(&["list-sessions", "-F", "#S"]).unwrap();
    assert_eq!(sessions.trim(), "fixture");

    let (_directory, provider, refused) = target(vec![
        Reply::ok("/api/profile", identity),
        Reply::post(
            "/api/profile/shells",
            503,
            json!({"detail":{"code":"shell_unavailable"}}),
        ),
    ]);
    assert_eq!(
        connect(refused).err().as_deref(),
        Some("The profile could not start this shell.")
    );
    provider.join().unwrap();
}
