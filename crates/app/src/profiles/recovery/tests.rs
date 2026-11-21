use super::*;
use crate::profiles::{Client, lifecycle::Operation, setup::Setup};
use agent_interface_roster::profiles::{operations, setup};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};

/// A profile the Rust service lists as serving.
fn entry() -> Value {
    json!({
        "state": "serving", "protocol_version": 1, "profile_id": "0a1b2c3d4e5f",
        "display_name": "Work", "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38"}, "socket": "/run/profile/api.sock",
        "tmux_socket": "/run/profile/tmux.sock", "credential_reference": {"type": "socket"}
    })
}
fn create() -> operations::Prepared {
    operations::Prepared::create("operation-create-a", "Work").unwrap()
}
fn prepared_setup() -> setup::Prepared {
    crate::profiles::tests::first_agent(
        &crate::profiles::tests::request(1),
        "0a1b2c3d4e5f",
        "guide",
    )
}
fn client() -> (tempfile::TempDir, Client) {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("provider");
    fs::write(&exe, "#!/bin/sh\nprintf invoked > \"$CALLS\"\nexit 9\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let mut client = Client::new(exe, "/usr/bin:/bin".into()).unwrap();
    assert_eq!(client.directory, std::env::current_dir().unwrap());
    client
        .environment
        .push(("CALLS".into(), dir.path().join("calls").into_os_string()));
    client.environment.push((
        "PRIVATE_CANARY".into(),
        "fixture-private-environment-value".into(),
    ));
    (dir, client)
}

#[test]
fn lifecycle_and_setup_recovery_match_kind_without_running_commands() {
    let (dir, client) = client();
    let operation = Operation::new(&client, create());
    let checkpoint = operation.checkpoint().unwrap();
    let restored = Operation::restore(&client, &checkpoint).unwrap();
    assert_eq!(restored.request_bytes(), operation.request_bytes());
    assert_eq!(restored.checkpoint().unwrap(), checkpoint);
    let setup = Setup::new(&client, prepared_setup());
    let setup_checkpoint = setup.checkpoint().unwrap();
    let restored = Setup::restore(&client, &setup_checkpoint).unwrap();
    for action in [setup::Action::FirstAgent, setup::Action::CancelFirstAgent] {
        assert_eq!(restored.bytes(action), setup.bytes(action));
    }
    assert!(matches!(
        Operation::restore(&client, &setup_checkpoint),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Setup::restore(&client, &checkpoint),
        Err(Error::Invalid)
    ));
    for bytes in [checkpoint, setup_checkpoint] {
        assert!(!String::from_utf8_lossy(&bytes).contains("fixture-private-environment-value"));
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_CANARY"));
    }
    assert!(!dir.path().join("calls").exists());
}

#[test]
fn recovery_survives_a_relaunch_from_another_environment() {
    let (dir, client) = client();
    let checkpoint = Operation::new(
        &client,
        operations::Prepared::create("create-1", "Work").unwrap(),
    )
    .checkpoint()
    .unwrap();
    let setup = Setup::new(&client, prepared_setup()).checkpoint().unwrap();
    let mut changed = client.clone();
    changed.environment.push(("HOME".into(), "changed".into()));
    changed.directory = dir.path().join("another");
    assert!(Operation::restore(&changed, &checkpoint).is_ok());
    assert!(Setup::restore(&changed, &setup).is_ok());
    assert!(!dir.path().join("calls").exists());
}

#[test]
fn malformed_and_oversized_capsules_are_rejected() {
    let (_dir, client) = client();
    let checkpoint = Operation::new(&client, create()).checkpoint().unwrap();
    for (key, value) in [
        ("version", json!(2)),
        ("kind", json!("unknown")),
        ("extra", json!(true)),
        ("state", json!("{}")),
    ] {
        let mut invalid: Value = serde_json::from_slice(&checkpoint).unwrap();
        invalid[key] = value;
        assert!(Operation::restore(&client, &serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    assert!(matches!(
        Operation::restore(&client, &vec![b' '; LIMIT + 1]),
        Err(Error::TooLarge)
    ));
    assert!(matches!(
        Operation::restore(&client, b"[]"),
        Err(Error::Invalid)
    ));
}

#[test]
fn discovery_and_restored_executors_use_the_captured_working_directory() {
    let (dir, mut client) = client();
    let origin = dir.path().join("origin");
    fs::create_dir(&origin).unwrap();
    client.directory = origin.canonicalize().unwrap();
    client.environment.push((
        "EXPECTED_CWD".into(),
        client.directory.as_os_str().to_owned(),
    ));
    let entry = entry();
    let list = json!({"profiles": [&entry]});
    let result = crate::profiles::tests::ready(&crate::profiles::tests::request(1));
    fs::write(
        &client.executable,
        format!(
            r#"#!/bin/sh
test "$(/bin/pwd)" = "$EXPECTED_CWD" || exit 8
printf '%s\n' "$*" >> "$CALLS"
case "$1 $2" in
 'profiles list') /bin/cat <<'FIXTURE'
{list}
FIXTURE
;;
 'profiles create') /bin/cat <<'FIXTURE'
{entry}
FIXTURE
;;
 'setup --file') /bin/cat "$3" >> "$CALLS.body"; printf '\n' >> "$CALLS.body"; /bin/cat <<'FIXTURE'
{result}
FIXTURE
;;
 *) exit 9;;
esac
"#
        ),
    )
    .unwrap();
    let operation = Operation::new(&client, create());
    let mut restored = Operation::restore(&client, &operation.checkpoint().unwrap()).unwrap();
    let setup = Setup::new(&client, prepared_setup());
    let restored_setup = Setup::restore(&client, &setup.checkpoint().unwrap()).unwrap();
    assert_eq!(client.list(&|| true).unwrap().len(), 1);
    // Later client changes do not mutate already captured or restored operations.
    client.directory = dir.path().to_owned();
    assert_eq!(
        restored.apply(&|| true).unwrap().status,
        operations::Status::Succeeded
    );
    assert!(
        restored_setup
            .run(setup::Action::FirstAgent, &|| true, |_| {})
            .unwrap()
            .ready_agent
            .is_some()
    );
    let bodies = fs::read_to_string(dir.path().join("calls.body")).unwrap();
    assert_eq!(
        bodies,
        format!(
            "{}\n",
            std::str::from_utf8(setup.bytes(setup::Action::FirstAgent)).unwrap()
        )
    );
    assert!(matches!(client.list(&|| true), Err(Error::CommandFailed)));
}
