//! Client coexistence checks. All clients and servers belong to the fixture;
//! ordinary clients run in the existing PTY harness, never a desktop terminal.

use std::time::{Duration, Instant};

use agent_interface_test_support::TmuxServer;

use super::connection::{Connection, Event, TIMEOUT, lines};
use super::pty::Reference;

fn request(client: &Connection, command: &str) -> String {
    let deadline = Instant::now() + TIMEOUT;
    let mut index = client.mark();
    client.send(&format!("{command}\n")).unwrap();
    loop {
        if let Event::Reply(reply) = client
            .next(
                &mut index,
                deadline.saturating_duration_since(Instant::now()),
            )
            .unwrap()
        {
            assert!(reply.ok, "{command}: {}", lines(&reply));
            return lines(&reply);
        }
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "fixture condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[track_caller]
fn describe(server: &TmuxServer, target: &str, format: &str) -> String {
    server
        .run(&["display-message", "-p", "-t", target, format])
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}

fn dimensions(server: &TmuxServer, expected: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let actual = describe(server, "fixture:0", "#{window_width}x#{window_height}");
        if actual == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected}, got {actual}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn control(server: &TmuxServer, session: &str) -> Connection {
    Connection::attach(
        server,
        &["attach-session", "-E", "-f", "ignore-size", "-t", session],
        &[],
    )
    .unwrap()
}

#[test]
#[ignore = "tmux 3.7c can crash during helper cleanup; run on disposable fixtures explicitly"]
fn observational_clients_and_grouped_helpers_preserve_source_lifetime() {
    source_lifetime(false);
}

#[test]
#[ignore = "reproduces a tmux 3.7c server crash; run on disposable fixtures explicitly"]
fn interrupted_helper_detach_preserves_source_lifetime() {
    source_lifetime(true);
}

fn source_lifetime(interrupt_detach: bool) {
    let server = TmuxServer::start().unwrap();
    server.run(&["set-option", "-g", "status", "off"]).unwrap();
    server
        .run(&["set-option", "-t", "fixture", "destroy-unattached", "off"])
        .unwrap();
    server
        .run(&["new-window", "-d", "-t", "fixture:1", "sleep 1000"])
        .unwrap();
    let identity = describe(
        &server,
        "fixture:0",
        "#{session_id}|#{window_id}|#{pane_pid}",
    );
    let ordinary = Reference::attach(&server, "fixture", 120, 40).unwrap();
    dimensions(&server, "120x40");
    let direct = control(&server, "fixture");
    server
        .run(&["new-session", "-d", "-t", "fixture", "-s", "helper"])
        .unwrap();
    let helper = control(&server, "helper");
    server
        .run(&[
            "set-option",
            "-t",
            "helper",
            "@agent_interface_attachment",
            "1",
        ])
        .unwrap();
    server
        .run(&["set-option", "-t", "helper", "destroy-unattached", "on"])
        .unwrap();

    request(&helper, "select-window -t helper:1");
    assert_eq!(describe(&server, "fixture", "#{window_index}"), "0");
    assert_eq!(describe(&server, "helper", "#{window_index}"), "1");
    assert_eq!(
        describe(&server, "fixture:1", "#{window_id}"),
        describe(&server, "helper:1", "#{window_id}")
    );
    for policy in ["smallest", "largest", "latest", "manual"] {
        server
            .run(&[
                "set-window-option",
                "-t",
                "fixture:0",
                "window-size",
                policy,
            ])
            .unwrap();
        // Only fixture-owned clients resize. Unsized control connections must
        // neither impose their default dimensions nor alter the source policy.
        if policy == "manual" {
            server
                .run(&["resize-window", "-t", "fixture:0", "-x", "120", "-y", "40"])
                .unwrap();
        }
        ordinary.resize(120, 40).unwrap();
        dimensions(&server, "120x40");
        assert_eq!(
            server
                .run(&[
                    "show-window-options",
                    "-v",
                    "-t",
                    "fixture:0",
                    "window-size"
                ])
                .unwrap()
                .trim(),
            policy
        );
    }
    if interrupt_detach {
        // Stop only this fixture's helper child so it cannot acknowledge detach.
        // Recalculation removes the unattached helper before its client exits.
        // Losing that child then exercises the server's interrupted-exit path.
        let pid = request(&helper, "display-message -p '#{client_pid}'");
        assert!(!pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()));
        let name = request(&helper, "display-message -p '#{client_name}'");
        assert!(
            std::process::Command::new("kill")
                .args(["-STOP", &pid])
                .status()
                .unwrap()
                .success()
        );
        let detached = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            request(
                &direct,
                &format!("detach-client -t {}", super::session::quote(&name)),
            );
            request(&direct, "refresh-client -C 120x40");
            wait_for(|| server.run(&["has-session", "-t", "=helper"]).is_err());
            // A missing helper is meaningful only while its source still lives.
            assert_eq!(
                describe(
                    &server,
                    "fixture:0",
                    "#{session_id}|#{window_id}|#{pane_pid}"
                ),
                identity
            );
        }));
        // Always release the stopped child before propagating a fixture panic.
        let killed = std::process::Command::new("kill")
            .args(["-KILL", &pid])
            .status()
            .unwrap();
        assert!(killed.success());
        if let Err(error) = detached {
            std::panic::resume_unwind(error);
        }
    }
    drop(helper);
    wait_for(|| server.run(&["has-session", "-t", "=helper"]).is_err());
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );
    assert_eq!(
        server
            .run(&["show-options", "-v", "-t", "fixture", "destroy-unattached"])
            .unwrap()
            .trim(),
        "off"
    );
    drop(direct);
    drop(ordinary);
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );
    let reconnected = control(&server, "fixture");
    for policy in ["smallest", "largest", "latest", "manual"] {
        server
            .run(&[
                "set-window-option",
                "-t",
                "fixture:0",
                "window-size",
                policy,
            ])
            .unwrap();
        dimensions(&server, "120x40");
    }
    assert_eq!(
        request(
            &reconnected,
            "display-message -p '#{session_id}|#{window_id}|#{pane_pid}'"
        ),
        identity
    );
    drop(reconnected);
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );
}

#[test]
fn sized_clients_share_geometry_even_through_groups_and_ignore_size() {
    for grouped in [false, true] {
        let server = TmuxServer::start().unwrap();
        server.run(&["set-option", "-g", "status", "off"]).unwrap();
        server
            .run(&["set-option", "-t", "fixture", "destroy-unattached", "off"])
            .unwrap();
        let ordinary = Reference::attach(&server, "fixture", 120, 40).unwrap();
        dimensions(&server, "120x40");
        let ordinary_name = server
            .run(&["list-clients", "-F", "#{client_name}"])
            .unwrap();
        let ordinary_name = ordinary_name.trim();
        assert!(!ordinary_name.is_empty() && !ordinary_name.contains('\n'));
        let session = if grouped {
            server
                .run(&["new-session", "-d", "-t", "fixture", "-s", "helper"])
                .unwrap();
            "helper"
        } else {
            "fixture"
        };
        let client = control(&server, session);
        let window = describe(&server, "fixture:0", "#{window_id}");
        for policy in ["smallest", "largest", "latest", "manual"] {
            if policy == "manual" {
                server
                    .run(&["resize-window", "-t", &window, "-x", "110", "-y", "35"])
                    .unwrap();
            }
            server
                .run(&["set-window-option", "-t", &window, "window-size", policy])
                .unwrap();
            for (ordinary_ignored, control_ignored) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let flag = |ignored| {
                    if ignored {
                        "ignore-size"
                    } else {
                        "!ignore-size"
                    }
                };
                server
                    .run(&[
                        "refresh-client",
                        "-t",
                        ordinary_name,
                        "-f",
                        flag(ordinary_ignored),
                    ])
                    .unwrap();
                request(
                    &client,
                    &format!("refresh-client -f {}", flag(control_ignored)),
                );
                // Selecting in this client's command context establishes it as
                // latest; a subsequent size declaration forces recalculation.
                request(&client, &format!("select-window -t {session}:0"));
                request(&client, "refresh-client -C 80x24");
                let expected = if policy == "manual" {
                    "110x35"
                } else if !ordinary_ignored && control_ignored {
                    "120x40"
                } else if ordinary_ignored && !control_ignored {
                    "80x24"
                } else if policy == "largest" {
                    "120x40"
                } else {
                    "80x24"
                };
                dimensions(&server, expected);
                eprintln!(
                    "grouped={grouped} policy={policy} ignore-size={ordinary_ignored}/{control_ignored}: {expected}"
                );
            }
            // A per-window control size is a shared cap, even for largest and
            // manual, rather than an independent viewport for this client.
            server
                .run(&["refresh-client", "-t", ordinary_name, "-f", "!ignore-size"])
                .unwrap();
            request(&client, "refresh-client -f !ignore-size");
            request(&client, &format!("refresh-client -C {window}:60x20"));
            dimensions(&server, "60x20");
            request(&client, &format!("refresh-client -C {window}:"));
            assert_eq!(
                server
                    .run(&["show-window-options", "-v", "-t", &window, "window-size"])
                    .unwrap()
                    .trim(),
                policy
            );
        }
        drop(client);
        if grouped {
            server.run(&["kill-session", "-t", "=helper"]).unwrap();
        }
        drop(ordinary);
        server.run(&["has-session", "-t", "=fixture"]).unwrap();
    }
}
