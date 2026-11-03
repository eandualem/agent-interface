use super::*;
use agent_interface_roster::profiles::{
    Profile,
    setup::{Agent, Brief},
};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn prepared() -> Prepared {
    let profile =
        Profile::parse(&serde_json::to_vec(&fixture()["discovery"]["show_ready"]).unwrap())
            .unwrap();
    Prepared::first_agent(
        "setup-a1",
        &profile,
        vec!["codex".into()],
        Agent {
            name: "worker".into(),
            directory: "/example/project".into(),
            create_directory: false,
            runtime: "codex".into(),
            model: None,
            brief: Brief::Text {
                text: "Project work".into(),
            },
        },
        false,
    )
    .unwrap()
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
    let fixture = fixture();
    let progress = &fixture["setup"]["progress"];
    let result = &fixture["setup"]["result"];
    let cancelled = &fixture["setup"]["cancellation_result"];
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
    assert_eq!(completed.ready_agent.as_deref(), Some("worker"));
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
    assert_eq!(
        bodies,
        [
            fixture["setup"]["first_agent"].clone(),
            fixture["setup"]["cancel_first_agent"].clone()
        ]
    );
}

#[test]
fn contradictory_exit_and_foreign_output_never_emit_terminal_success() {
    let fixture = fixture();
    for (key, exit, accepted) in [
        ("result", 0, true),
        ("result", 1, false),
        ("cancellation_unsafe", 3, true),
        ("failed_before_resolution", 1, true),
    ] {
        let action = if key == "cancellation_unsafe" {
            Action::CancelFirstAgent
        } else {
            Action::FirstAgent
        };
        let body = &fixture["setup"][key];
        let (_dir, client) = stub(&format!(
            "/bin/cat <<'FIXTURE'\n{body}\nFIXTURE\nexit {exit}"
        ));
        let result =
            Setup::new(&client, prepared()).run(action, &|| true, |_| panic!("terminal callback"));
        assert_eq!(result.is_ok(), accepted, "{key}/{exit}");
    }
    let mut foreign = fixture["setup"]["result"].clone();
    foreign["lifecycle_id"] = "another".into();
    let (_dir, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{foreign}\nFIXTURE"));
    assert!(
        Setup::new(&client, prepared())
            .run(Action::FirstAgent, &|| true, |_| panic!("foreign event"))
            .is_err()
    );
}

#[test]
fn uncertain_setup_retries_exact_bytes_and_observation_stop_is_not_cancellation() {
    let result = &fixture()["setup"]["result"];
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
    let fixture = fixture();
    let progress = &fixture["setup"]["progress"];
    let result = &fixture["setup"]["result"];
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
