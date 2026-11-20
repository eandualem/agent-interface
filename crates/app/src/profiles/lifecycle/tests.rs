use super::*;
use profiles::operations::Status;
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt};

/// The service's answer to a create, as its `machine-profile-created`
/// fixture has it.
fn entry(name: &str) -> Value {
    serde_json::json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": "0a1b2c3d4e5f", "display_name": name,
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": "/home/user/.local/share/backbone/profiles/0a1b2c3d4e5f/api.sock",
        "tmux_socket": "/home/user/.local/share/backbone/profiles/0a1b2c3d4e5f/tmux.sock",
        "credential_reference": {"type": "socket"}
    })
}
fn create() -> Prepared {
    Prepared::create("operation-create-a", "Work").unwrap()
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
fn a_create_keeps_its_original_caller() {
    let (dir, mut client) = stub(&format!(
        r#"
printf '%s\n' "$*" >> "$CAPTURE"
test "$BACKBONE_AGENT" = original || exit 9
/bin/cat <<'FIXTURE'
{}
FIXTURE
"#,
        entry("Work")
    ));
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "original".into()));
    let mut operation = Operation::new(&client, create());
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "replacement".into()));
    assert_eq!(operation.apply(&|| true).unwrap().status, Status::Succeeded);
    assert_eq!(
        fs::read_to_string(dir.path().join("calls")).unwrap(),
        "profiles create --starter core-v1 --request-id operation-create-a --json -- Work\n"
    );
}

#[test]
fn a_timeout_then_asking_again_resolves_the_original_request() {
    let (dir, client) = stub(&format!(
        r#"
printf '%s\n' "$2" >> "$CAPTURE"
if [ ! -f "$CAPTURE.once" ]; then : > "$CAPTURE.once"; exec /bin/sleep 5; fi
/bin/cat <<'FIXTURE'
{}
FIXTURE
"#,
        entry("Work")
    ));
    let mut operation = Operation::new(&client, create());
    let original = operation.request_bytes().to_vec();
    assert_eq!(
        operation.apply_with_timeout(&|| true, Duration::from_secs(2)),
        Err(Error::TimedOut)
    );
    assert_eq!(operation.request_bytes(), original);
    assert_eq!(
        operation.refresh(&|| true).unwrap().status,
        Status::Succeeded
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("calls")).unwrap(),
        "create\ncreate\n"
    );
}

#[test]
fn service_descendants_do_not_hold_receipt_open_or_get_killed() {
    let success = entry("Work");
    let (dir, client) = stub(&format!(
        r#"
/bin/sh -c '/bin/sleep 0.2; printf "service output\n"; printf alive > "$CAPTURE.alive"; exec /bin/sleep 10' &
printf '%s' "$!" > "$CAPTURE.pid"
/bin/cat <<'FIXTURE'
{success}
FIXTURE
"#
    ));
    let started = Instant::now();
    let mut operation = Operation::new(&client, create());
    let result = operation.apply_with_timeout(&|| true, Duration::from_secs(2));
    let pid: i32 = fs::read_to_string(dir.path().join("calls.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.path().join("calls.alive").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let wrote_after_return = dir.path().join("calls.alive").exists();
    let alive = rustix::process::test_kill_process(pid);
    // Fixture owns this disposable child; always clean it before assertions.
    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    assert!(alive.is_ok());
    assert!(
        wrote_after_return,
        "service must survive writing after receipt completion"
    );
    assert_eq!(result.unwrap().status, Status::Succeeded);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn cancelled_and_oversized_invocations_preserve_original_request() {
    let (dir, client) =
        stub("printf called > \"$CAPTURE\"; exec /usr/bin/head -c 1048577 /dev/zero");
    let mut operation = Operation::new(&client, create());
    let original = operation.request_bytes().to_vec();
    assert_eq!(operation.apply(&|| false), Err(Error::Cancelled));
    assert!(!dir.path().join("calls").exists());
    assert_eq!(operation.apply(&|| true), Err(Error::TooLarge));
    assert_eq!(operation.request_bytes(), original);
}

#[test]
fn the_rust_service_makes_a_profile_under_the_requests_own_id() {
    let entry = entry("-Work");
    let (dir, client) = stub(&format!(
        r#"
printf '%s\n' "$*" >> "$CAPTURE"
case "$2" in
  list) if test -f "$CAPTURE.listed"; then printf '{{"profiles":[%s]}}\n' "$(/bin/cat "$CAPTURE.listed")"; else printf '{{"profiles":[]}}\n'; fi;;
  create) test -f "$CAPTURE.refuse" && exit 1; test -f "$CAPTURE.unknown" && exit 4
test -f "$CAPTURE.unreachable" && exit 3
/bin/cat <<'FIXTURE'
{entry}
FIXTURE
;;
  *) exit 7;;
esac
"#
    ));
    let request = "0190c3a2c1107a3b8c9d0e1f2a3b4c5d";
    let calls = dir.path().join("calls");
    let create =
        format!("profiles create --starter core-v1 --request-id {request} --json -- -Work");
    let prepared = || Prepared::create(request, "-Work").unwrap();

    // No answer: the outcome is unknown, and asking again sends the same ID.
    fs::write(dir.path().join("calls.unknown"), "").unwrap();
    let mut operation = Operation::new(&client, prepared());
    assert!(matches!(
        operation.apply(&|| true),
        Err(Error::CommandFailed)
    ));
    fs::remove_file(dir.path().join("calls.unknown")).unwrap();
    // A repeat that cannot reach the service says nothing of the first send.
    fs::write(dir.path().join("calls.unreachable"), "").unwrap();
    assert!(matches!(
        operation.retry(&|| true),
        Err(Error::CommandFailed)
    ));
    fs::remove_file(dir.path().join("calls.unreachable")).unwrap();
    let receipt = operation.refresh(&|| true).unwrap();
    assert_eq!(receipt.status, Status::Succeeded);
    assert_eq!(
        receipt.identity.unwrap().profile_id,
        "0a1b2c3d4e5f",
        "the profile the service made"
    );
    assert_eq!(
        fs::read_to_string(&calls).unwrap(),
        format!("{create}\n").repeat(3)
    );

    // A refused repeat settles only when no profile has the name.
    let mut repeated = Operation::new(&client, prepared());
    fs::write(dir.path().join("calls.refuse"), "").unwrap();
    fs::write(dir.path().join("calls.listed"), entry.to_string()).unwrap();
    assert!(matches!(
        repeated.retry(&|| true),
        Err(Error::CommandFailed)
    ));
    // A failed entry with no name may be the one made.
    let unnamed = serde_json::json!({"state": "failed", "profile_id": "0a1b2c3d4e5f",
        "code": "database_unreachable", "reason": "down"});
    fs::write(dir.path().join("calls.listed"), unnamed.to_string()).unwrap();
    assert!(matches!(
        repeated.retry(&|| true),
        Err(Error::CommandFailed)
    ));
    fs::remove_file(dir.path().join("calls.listed")).unwrap();
    let receipt = repeated.retry(&|| true).unwrap();
    assert_eq!(receipt.status, Status::Failed);
    fs::remove_file(dir.path().join("calls.refuse")).unwrap();

    // A refusal made nothing, so a new request is safe.
    fs::write(dir.path().join("calls.refuse"), "").unwrap();
    let mut refused = Operation::new(&client, prepared());
    let receipt = refused.apply(&|| true).unwrap();
    assert_eq!(receipt.status, Status::Failed);
    assert_eq!(receipt.error_code.as_deref(), Some("refused"));
}
