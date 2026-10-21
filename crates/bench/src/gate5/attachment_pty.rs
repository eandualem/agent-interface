//! Ordinary-client attachment alternative for ADRs 0003 and 0015.
//! Only owned private fixtures receive input; no desktop terminal is involved.

use std::time::{Duration, Instant};

use tmux_workspaces_term::{Screen, Terminal};
use tmux_workspaces_test_support::TmuxServer;

use super::{connection::TIMEOUT, pty::Reference, session::quote};

#[track_caller]
fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !ready() {
        assert!(Instant::now() < deadline, "fixture condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn describe(server: &TmuxServer, target: &str, format: &str) -> String {
    server
        .run(&["display-message", "-p", "-t", target, format])
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}

/// Decode the PTY stream with the native terminal core, retaining parser state.
fn screen_with(client: &Reference, terminal: &mut Terminal, offset: &mut usize, text: &str) {
    let mut drawn = String::new();
    wait_for(|| {
        let bytes = client.bytes_from(*offset);
        *offset += bytes.len();
        terminal.write(&bytes);
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        drawn.clear();
        for row in 0..screen.rows() {
            for col in 0..screen.cols() {
                drawn.push_str(screen.cell(row, col).unwrap().0);
            }
            drawn.push('\n');
        }
        drawn.contains(text)
    });
    eprintln!("native grid contains {text:?}");
}

#[test]
fn ordinary_client_preserves_prefix_modes_and_session_identity() {
    let server = TmuxServer::start().unwrap();
    let files = tempfile::tempdir().unwrap();
    let input = files.path().join("input");
    // The pane records raw application input so prefix/menu keys cannot silently
    // pass through to the program while the tmux-side assertions still succeed.
    server
        .run(&[
            "respawn-pane",
            "-k",
            "-t",
            "fixture:0",
            &format!(
                "stty raw -echo; printf 'PTY attachment ready'; cat > {}",
                quote(input.to_str().unwrap())
            ),
        ])
        .unwrap();
    server
        .run(&["set-option", "-t", "fixture", "destroy-unattached", "off"])
        .unwrap();
    server
        .run(&["new-window", "-d", "-t", "fixture:1", "sleep 1000"])
        .unwrap();
    server
        .run(&["new-session", "-d", "-t", "fixture", "-s", "helper"])
        .unwrap();
    // Automatic helper cleanup is a separate, unresolved lifecycle check.
    server
        .run(&["set-option", "-t", "helper", "destroy-unattached", "off"])
        .unwrap();
    // Synthetic key bursts can trigger tmux's timing-based paste detection.
    // This fixture exercises bindings, not that independent input heuristic.
    server
        .run(&["set-option", "-t", "helper", "assume-paste-time", "0"])
        .unwrap();
    server
        .run(&["set-option", "-g", "status-left", "PTY fixture"])
        .unwrap();
    server
        .run(&["set-option", "-g", "status-right", ""])
        .unwrap();
    server
        .run(&[
            "bind-key",
            "-T",
            "prefix",
            "x",
            "set-option",
            "-t",
            "helper",
            "@prefix-probe",
            "hit",
        ])
        .unwrap();
    server
        .run(&[
            "bind-key",
            "-T",
            "prefix",
            "p",
            "command-prompt",
            "-p",
            "Probe prompt",
            "set-option -t helper @prompt-probe '%%'",
        ])
        .unwrap();
    server
        .run(&[
            "bind-key",
            "-T",
            "prefix",
            "m",
            "display-menu",
            "-T",
            "Probe menu",
            "Choose marker",
            "x",
            "set-option -t helper @menu-probe chosen",
        ])
        .unwrap();
    server
        .run(&[
            "set-environment",
            "-t",
            "helper",
            "DISPLAY",
            "fixture-display",
        ])
        .unwrap();
    let identity = describe(
        &server,
        "fixture:0",
        "#{session_id}|#{window_id}|#{pane_pid}",
    );
    let client = Reference::attach(&server, "helper", 80, 24).unwrap();
    let mut terminal = Terminal::new(80, 24, 0).unwrap();
    let mut offset = 0;
    screen_with(&client, &mut terminal, &mut offset, "PTY attachment ready");
    assert_eq!(
        server
            .run(&["show-environment", "-t", "helper", "DISPLAY"])
            .unwrap()
            .trim(),
        "DISPLAY=fixture-display"
    );
    let option = |name| {
        server
            .run(&["show-options", "-v", "-t", "helper", name])
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    client.send(b"\x02x").unwrap();
    wait_for(|| option("@prefix-probe") == "hit");
    client.send(b"\x02p").unwrap();
    screen_with(&client, &mut terminal, &mut offset, "Probe prompt");
    client.send(b"answer\r").unwrap();
    wait_for(|| option("@prompt-probe") == "answer");
    client.send(b"\x02m").unwrap();
    screen_with(&client, &mut terminal, &mut offset, "Choose marker");
    client.send(b"x").unwrap();
    wait_for(|| option("@menu-probe") == "chosen");

    client.send(b"\x02[").unwrap();
    wait_for(|| describe(&server, "helper:0", "#{pane_in_mode}") == "1");
    screen_with(&client, &mut terminal, &mut offset, "[0/0]");
    client.send(b"q").unwrap();
    wait_for(|| describe(&server, "helper:0", "#{pane_in_mode}") == "0");
    assert!(std::fs::read(&input).unwrap().is_empty());
    client.send(b"PAYLOAD").unwrap();
    wait_for(|| std::fs::read(&input).unwrap() == b"PAYLOAD");
    client.send(b"\x02\x02").unwrap();
    wait_for(|| std::fs::read(&input).unwrap() == b"PAYLOAD\x02");

    client.send(b"\x02n").unwrap();
    wait_for(|| describe(&server, "helper", "#{window_index}") == "1");
    assert_eq!(describe(&server, "fixture", "#{window_index}"), "0");
    // A custom prompt owns p above; use the unmodified last-window binding.
    client.send(b"\x02l").unwrap();
    wait_for(|| describe(&server, "helper", "#{window_index}") == "0");
    client.send(b"\x02d").unwrap();
    wait_for(|| {
        server
            .run(&["list-clients", "-t", "helper", "-F", "#{client_pid}"])
            .unwrap()
            .is_empty()
    });
    drop(client);
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );

    let reconnected = Reference::attach(&server, "helper", 80, 24).unwrap();
    let mut terminal = Terminal::new(80, 24, 0).unwrap();
    screen_with(&reconnected, &mut terminal, &mut 0, "PTY attachment ready");
    reconnected.send(b"AGAIN").unwrap();
    wait_for(|| std::fs::read(&input).unwrap() == b"PAYLOAD\x02AGAIN");
    reconnected.send(b"\x02d").unwrap();
    wait_for(|| {
        server
            .run(&["list-clients", "-t", "helper", "-F", "#{client_pid}"])
            .unwrap()
            .is_empty()
    });
    drop(reconnected);
    server.run(&["kill-session", "-t", "=helper"]).unwrap();
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );
    assert_eq!(std::fs::read(&input).unwrap(), b"PAYLOAD\x02AGAIN");
}
