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
        let identity = json!({"protocol_version":1,"profile_id":format!("profile-{label}"),
            "instance_id":format!("instance-{label}"), "machine":{"id":format!("machine-{label}")}});
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
                        remote_profile: None,
                        session: "fixture".into(),
                        source_socket: None,
                    }),
                },
                directory: None,
                // Even an available same-name target on the window's own server must not receive I/O.
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
fn a_new_bound_pane_asks_for_the_profile_home_and_a_saved_folder_is_kept() {
    let leaf = |cwd: Option<&str>| Leaf {
        id: "123456abcdef".into(),
        cwd: cwd.map(str::to_owned),
        empty: false,
        view: None,
        attachment: None,
    };
    assert_eq!(bound_shell_folder(&leaf(Some("/"))), None);
    assert_eq!(bound_shell_folder(&leaf(None)), None);
    assert_eq!(bound_shell_folder(&leaf(Some("/work"))), Some("/work"));
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
                source: Err("A bound shell must not use the window's own source.".into()),
            },
        )
    };
    let identity = json!({"protocol_version":1,"profile_id":"profile-a",
        "instance_id":"instance-a","machine":{"id":"machine-a"}});
    let shell = json!({"schema_version":1,"profile_id":"profile-a",
        "instance_id":"instance-a",
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

#[test]
fn a_run_view_on_its_profile_server_is_shown_read_only() {
    use crate::view::backend::{Backend, READ_ONLY, ReadOnly};
    use agent_interface_term::input::{Button, Input, KeyPress, MouseAction, Pointer, Scroll};
    const VIEW: &str = "run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60";
    let server = TmuxServer::start().unwrap();
    server
        .run(&[
            "new-session",
            "-d",
            "-s",
            VIEW,
            "sh",
            "-c",
            // A program that tracks the mouse, as a review's TUI does.
            "printf 'run-output\\n\\033[?1000h'; exec cat",
        ])
        .unwrap();
    let socket: PathBuf = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap()
        .trim()
        .into();
    let identity = json!({"protocol_version":1,"profile_id":"profile-a",
        "instance_id":"instance-a","machine":{"id":"machine-a"}});
    let (_directory, connection, provider) = provider_with_tmux(
        "a",
        (0..2)
            .map(|_| Reply::ok("/api/profile", identity.clone()))
            .collect(),
        Some(socket),
    );
    let target = Target {
        connection: Some(connection),
        leaf: Leaf {
            id: "123456abcdef".into(),
            cwd: None,
            empty: false,
            view: None,
            attachment: Some(Attachment {
                remote_profile: None,
                session: VIEW.into(),
                source_socket: None,
            }),
        },
        directory: None,
        source: Err("bound".into()),
    };
    assert!(target.is_run_view());
    // The same name on a server without a profile is a person's own session.
    let mut own = target.clone();
    own.connection = None;
    assert!(!own.is_run_view());
    let ready = connect(target).unwrap();
    provider.join().unwrap();
    let view = ReadOnly(Arc::clone(ready.panes.as_ref().unwrap()));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let tracking = server
            .run(&[
                "display-message",
                "-p",
                "-t",
                VIEW,
                "#{mouse_standard_flag}",
            ])
            .unwrap();
        if tracking.trim() == "1"
            && let Some(update) = view.take_update()
        {
            // Clicks stay with the viewer, so a plain drag selects.
            assert!(update.screens.iter().all(|(_, s)| !s.mouse_input()));
            break;
        }
        assert!(Instant::now() < deadline, "no update with tracking on");
        std::thread::sleep(Duration::from_millis(5));
    }
    let key = Input::Key(KeyPress {
        key: "t".into(),
        text: Some("typed".into()),
        ..KeyPress::default()
    });
    for refused in [key, Input::Paste("typed\r".into())] {
        let error = view.send_reserved_input(ready.input, refused, None);
        assert_eq!(error.unwrap_err().to_string(), READ_ONLY);
    }
    let pointer = Pointer::default();
    for kept in [
        Input::Scroll(Scroll::Lines(-1)),
        Input::Wheel(pointer, -1),
        Input::Click(pointer, Button::Left),
        Input::Mouse(pointer, MouseAction::Press(Button::Left)),
    ] {
        view.send_reserved_input(ready.input, kept, None).unwrap();
    }
    let shown = server.run(&["capture-pane", "-p", "-t", VIEW]).unwrap();
    assert!(!shown.contains("typed"), "{shown}");
}
