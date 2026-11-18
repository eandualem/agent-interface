use super::*;
use crate::profiles::tests::{answer, cancelled, first_agent, ready, request, step};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt};

fn id() -> String {
    request(1)
}
fn prepared() -> Prepared {
    first_agent(&id(), "0a1b2c3d4e5f", "guide")
}
fn stub(script: &str) -> (tempfile::TempDir, Client) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("provider");
    fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    client
        .environment
        .push(("CAPTURE".into(), dir.path().join("calls").into_os_string()));
    client
        .environment
        .push(("HOME".into(), dir.path().as_os_str().to_owned()));
    (dir, client)
}

#[test]
fn setup_and_cancellation_preserve_choices_caller_and_terminal_authority() {
    let progress = step(&id(), 1, "preflight");
    let result = ready(&id());
    let cancelled = cancelled(&id());
    let (dir, mut client) = stub(&format!(
        r#"
test "$BACKBONE_AGENT" = original || exit 9
test "$1" = setup && test "$2" = --file && test "$4" = --jsonl || exit 8
/bin/cat "$3" >> "$CAPTURE.body"; printf '\n' >> "$CAPTURE.body"
case "$(/bin/cat "$3")" in
  *'"action":"cancel_first_agent"'*) /bin/cat <<'FIXTURE'
{cancelled}
FIXTURE
;;
  *) /bin/cat <<'FIXTURE'
{progress}
{result}
FIXTURE
;;
esac
"#
    ));
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "original".into()));
    let setup = Setup::new(&client, prepared());
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "replacement".into()));
    let mut events = Vec::new();
    let completed = setup
        .run(Action::FirstAgent, &|| true, |event| events.push(event))
        .unwrap();
    assert_eq!(completed.ready_agent.as_deref(), Some("guide"));
    assert_eq!(events.len(), 1);
    assert!(events[0].step.is_some() && events[0].ready_agent.is_none());
    let cancelled = setup
        .run(Action::CancelFirstAgent, &|| true, |_| {
            panic!("terminal emitted early")
        })
        .unwrap();
    assert!(cancelled.cancelled);
    let recovered = Setup::restore(&setup.client, &setup.checkpoint().unwrap()).unwrap();
    for (action, terminal) in [
        (Action::FirstAgent, &completed),
        (Action::CancelFirstAgent, &cancelled),
    ] {
        let bytes = setup.outcome_checkpoint(terminal).unwrap();
        assert_eq!(
            &recovered.restore_outcome(action, &bytes).unwrap(),
            terminal
        );
    }
    assert!(setup.outcome_checkpoint(&events[0]).is_err());
    let bodies = fs::read_to_string(dir.path().join("calls.body")).unwrap();
    let bodies: Vec<Value> = bodies
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let prepared = prepared();
    let sent = |action| serde_json::from_slice::<Value>(prepared.bytes(action)).unwrap();
    assert_eq!(
        bodies,
        [sent(Action::FirstAgent), sent(Action::CancelFirstAgent)]
    );
}

#[test]
fn contradictory_exit_and_foreign_output_never_emit_terminal_success() {
    let unsafe_cancel = answer(
        &id(),
        "cancel_first_agent",
        "action_required",
        "cancellation_unsafe",
        "none",
    );
    let refused = answer(
        &id(),
        "first_agent",
        "failed",
        "invalid_request",
        "new_request",
    );
    let paused = answer(&id(), "first_agent", "paused", "new_code", "later");
    for (body, exit, accepted) in [
        (ready(&id()), 0, true),
        (ready(&id()), 1, false),
        (unsafe_cancel, 3, true),
        (refused.clone(), 1, true),
        // An invalid request exits 2.
        (refused, 2, true),
        // A status this app doesn't know is received whatever the exit.
        (paused, 1, true),
    ] {
        let action = if body["action"] == "cancel_first_agent" {
            Action::CancelFirstAgent
        } else {
            Action::FirstAgent
        };
        let (_dir, client) = stub(&format!(
            "/bin/cat <<'FIXTURE'\n{body}\nFIXTURE\nexit {exit}"
        ));
        let result =
            Setup::new(&client, prepared()).run(action, &|| true, |_| panic!("terminal callback"));
        assert_eq!(result.is_ok(), accepted, "{body}/{exit}");
    }
    let mut foreign = ready(&id());
    foreign["request_id"] = request(2).into();
    let (_dir, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{foreign}\nFIXTURE"));
    assert!(
        Setup::new(&client, prepared())
            .run(Action::FirstAgent, &|| true, |_| panic!("foreign event"))
            .is_err()
    );
}

#[test]
fn uncertain_setup_retries_exact_bytes_and_observation_stop_is_not_cancellation() {
    let result = ready(&id());
    let (dir, client) = stub(&format!(
        r#"
/bin/cat "$3" >> "$CAPTURE.body"; printf '\n' >> "$CAPTURE.body"
if [ ! -e "$CAPTURE" ]; then printf started > "$CAPTURE"; exec /bin/sleep 10; fi
/bin/cat <<'FIXTURE'
{result}
FIXTURE
"#
    ));
    let setup = Setup::new(&client, prepared());
    let original = setup.bytes(Action::FirstAgent).to_vec();
    assert_eq!(
        setup.run(Action::FirstAgent, &|| false, |_| {}),
        Err(Error::Cancelled)
    );
    assert!(!dir.path().join("calls.body").exists());
    assert_eq!(
        setup.run_with_timeout(Action::FirstAgent, &|| true, |_| {}, Duration::from_secs(2)),
        Err(Error::TimedOut)
    );
    assert_eq!(setup.bytes(Action::FirstAgent), original);
    assert!(
        setup
            .run(Action::FirstAgent, &|| true, |_| {})
            .unwrap()
            .ready_agent
            .is_some()
    );
    let bodies = fs::read_to_string(dir.path().join("calls.body")).unwrap();
    assert_eq!(
        bodies,
        format!("{0}\n{0}\n", String::from_utf8(original).unwrap())
    );
}

#[test]
fn progress_callback_cannot_extend_terminal_observation_deadline() {
    let progress = step(&id(), 1, "preflight");
    let result = ready(&id());
    let (_dir, client) = stub(&format!(
        "/bin/cat <<'FIXTURE'\n{progress}\n{result}\nFIXTURE"
    ));
    let mut observed = false;
    let result = Setup::new(&client, prepared()).run_with_timeout(
        Action::FirstAgent,
        &|| true,
        |_| {
            observed = true;
            std::thread::sleep(Duration::from_secs(2));
        },
        Duration::from_secs(1),
    );
    assert!(observed);
    assert_eq!(result, Err(Error::TimedOut));
}

#[test]
fn a_served_profile_reads_the_rust_services_runtimes_without_setup_flags() {
    let (_dir, client) = stub(
        r#"[ "$*" = "runtimes --json" ] || exit 2
printf '%s' '{"schema_version":1,"items":[{"id":"claude","display_name":"Claude Code","available":true,"setup_supported":true,"reports_state":"hooks + terminal","models":["opus"],"efforts":[],"unattended":{"supported":false,"args":[],"sandboxed":null},"authentication":"unknown"}]}'"#,
    );
    let identity = agent_interface_roster::profiles::Identity {
        profile_id: "0a1b2c3d4e5f".into(),
        installation_id: "5f2c9e0d1b7a4e38".into(),
        lifecycle_id: "5f2c9e0d1b7a4e38".into(),
    };
    let catalog = client.served_runtimes(&identity, &|| true).unwrap();
    assert_eq!(catalog.identity(), &identity);
    assert!(catalog.items()[0].setup_compatibility.compatible);
}
