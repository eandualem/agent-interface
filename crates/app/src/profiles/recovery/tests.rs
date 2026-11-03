use super::*;
use crate::profiles::{lifecycle::Operation, setup::Setup};
use agent_interface_roster::profiles::{Profile, operations, setup};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn profile() -> Profile {
    Profile::parse(&serde_json::to_vec(&fixture()["discovery"]["show_ready"]).unwrap()).unwrap()
}
fn prepared_setup() -> setup::Prepared {
    setup::Prepared::first_agent(
        "setup-a1",
        &profile(),
        vec!["codex".into()],
        setup::Agent {
            name: "worker".into(),
            directory: "/example/project".into(),
            create_directory: false,
            runtime: "codex".into(),
            model: None,
            brief: setup::Brief::Text {
                text: "Project work".into(),
            },
        },
        false,
    )
    .unwrap()
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
fn lifecycle_and_setup_recovery_match_kind_and_context_without_running_commands() {
    let (dir, client) = client();
    let operation = Operation::new(
        &client,
        operations::Prepared::start("operation-start-a", &profile()).unwrap(),
    );
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
fn any_effective_caller_context_change_refuses_recovery_before_execution() {
    let (dir, client) = client();
    let checkpoint = Operation::new(
        &client,
        operations::Prepared::create("create-1", "Work", "baseline").unwrap(),
    )
    .checkpoint()
    .unwrap();
    let setup = Setup::new(&client, prepared_setup()).checkpoint().unwrap();
    let mut variants = Vec::new();
    for key in [
        "HOME",
        "PATH",
        "BACKBONE_AGENT",
        "BACKBONE_API_KEY",
        "SESSION_METADATA",
    ] {
        let mut changed = client.clone();
        changed.environment.push((key.into(), "changed".into()));
        variants.push(changed);
    }
    let mut changed = client.clone();
    changed.directory = dir.path().join("another");
    variants.push(changed);
    let mut changed = client.clone();
    changed.executable = dir.path().join("another");
    variants.push(changed);
    for changed in variants {
        assert!(matches!(
            Operation::restore(&changed, &checkpoint),
            Err(Error::ContextChanged)
        ));
        assert!(matches!(
            Setup::restore(&changed, &setup),
            Err(Error::ContextChanged)
        ));
    }
    assert!(!dir.path().join("calls").exists());
}

#[test]
fn fingerprint_uses_effective_values_and_unambiguous_raw_fields() {
    let (_dir, mut client) = client();
    client.environment = vec![
        ("B".into(), "first".into()),
        ("A".into(), "one".into()),
        ("B".into(), "two".into()),
    ];
    let first = client.context_fingerprint();
    client.environment = vec![("B".into(), "two".into()), ("A".into(), "one".into())];
    assert_eq!(client.context_fingerprint(), first);
    client.environment = vec![("AB".into(), "C".into())];
    let joined = client.context_fingerprint();
    client.environment = vec![("A".into(), "BC".into())];
    assert_ne!(client.context_fingerprint(), joined);
    use std::os::unix::ffi::OsStringExt;
    client.environment = vec![("RAW".into(), std::ffi::OsString::from_vec(vec![0xff]))];
    let raw = client.context_fingerprint();
    client.environment = vec![("RAW".into(), "�".into())];
    assert_ne!(client.context_fingerprint(), raw);
}

#[test]
fn malformed_and_oversized_capsules_are_rejected() {
    let (_dir, client) = client();
    let checkpoint = Operation::new(
        &client,
        operations::Prepared::start("operation-start-a", &profile()).unwrap(),
    )
    .checkpoint()
    .unwrap();
    for (key, value) in [
        ("version", json!(2)),
        ("kind", json!("unknown")),
        ("context", json!(null)),
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
    let fixture = fixture();
    let list = &fixture["discovery"]["list"];
    let receipt = &fixture["lifecycle"]["succeeded"];
    let result = &fixture["setup"]["result"];
    fs::write(&client.executable,format!(r#"#!/bin/sh
test "$(/bin/pwd)" = "$EXPECTED_CWD" || exit 8
printf '%s\n' "$*" >> "$CALLS"
case "$1 $2" in
 'profiles list') /bin/cat <<'FIXTURE'
{list}
FIXTURE
;;
 'profiles apply') /bin/cat "$4" >> "$CALLS.body"; printf '\n' >> "$CALLS.body"; /bin/cat <<'FIXTURE'
{receipt}
FIXTURE
;;
 'setup --file') /bin/cat "$3" >> "$CALLS.body"; printf '\n' >> "$CALLS.body"; /bin/cat <<'FIXTURE'
{result}
FIXTURE
;;
 *) exit 9;;
esac
"#)).unwrap();
    let operation = Operation::new(
        &client,
        operations::Prepared::start("operation-start-a", &profile()).unwrap(),
    );
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
            "{}\n{}\n",
            std::str::from_utf8(operation.request_bytes()).unwrap(),
            std::str::from_utf8(setup.bytes(setup::Action::FirstAgent)).unwrap()
        )
    );
    assert!(matches!(client.list(&|| true), Err(Error::CommandFailed)));
}
