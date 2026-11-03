use super::*;
use profiles::{Profile, operations::Status};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn start() -> Prepared {
    let profile =
        Profile::parse(&serde_json::to_vec(&fixture()["discovery"]["show_ready"]).unwrap())
            .unwrap();
    Prepared::start("operation-start-a", &profile).unwrap()
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
fn apply_and_lookup_keep_exact_request_and_original_caller() {
    let success = &fixture()["lifecycle"]["succeeded"];
    let (dir, mut client) = stub(&format!(
        r#"
printf '%s\n' "$*" >> "$CAPTURE"
test "$BACKBONE_AGENT" = original || exit 9
case "$2" in
  apply) /bin/cat "$4" >> "$CAPTURE.body"; printf '\n' >> "$CAPTURE.body";;
  operation) test "$*" = 'profiles operation --json -- operation-start-a' || exit 8;;
  *) exit 7;;
esac
/bin/cat <<'FIXTURE'
{success}
FIXTURE
"#
    ));
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "original".into()));
    let mut operation = Operation::new(&client, start());
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "replacement".into()));
    let body = operation.request_bytes().to_vec();
    assert_eq!(operation.apply(&|| true).unwrap().status, Status::Succeeded);
    assert_eq!(
        operation.refresh(&|| true).unwrap().status,
        Status::Succeeded
    );
    assert_eq!(operation.apply(&|| true).unwrap().status, Status::Succeeded);
    let captured = fs::read_to_string(dir.path().join("calls.body")).unwrap();
    assert_eq!(
        captured,
        format!("{0}\n{0}\n", String::from_utf8(body.clone()).unwrap())
    );
    assert_eq!(operation.request_bytes(), body);
    assert_eq!(operation.request_id(), "operation-start-a");
    assert_eq!(
        fs::read_to_string(dir.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}

#[test]
fn timeout_then_lookup_resolves_original_operation_without_resubmitting() {
    let success = &fixture()["lifecycle"]["succeeded"];
    let (dir, client) = stub(&format!(
        r#"
printf '%s\n' "$2" >> "$CAPTURE"
if [ "$2" = apply ]; then exec /bin/sleep 5; fi
/bin/cat <<'FIXTURE'
{success}
FIXTURE
"#
    ));
    let mut operation = Operation::new(&client, start());
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
        "apply\noperation\n"
    );
}

#[test]
fn failed_receipts_are_retained_but_nonzero_success_and_foreign_receipts_are_not() {
    let fixture = fixture();
    let failed = &fixture["lifecycle"]["failed"];
    let (_dir, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{failed}\nFIXTURE\nexit 1"));
    assert_eq!(
        Operation::new(&client, start())
            .apply(&|| true)
            .unwrap()
            .status,
        Status::Failed
    );
    let success = &fixture["lifecycle"]["succeeded"];
    let (_dir, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{success}\nFIXTURE\nexit 1"));
    let mut operation = Operation::new(&client, start());
    assert_eq!(operation.apply(&|| true), Err(Error::CommandFailed));
    let mut foreign = success.clone();
    foreign["lifecycle_id"] = "replacement".into();
    let (_dir, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{foreign}\nFIXTURE"));
    let mut operation = Operation::new(&client, start());
    assert_eq!(
        operation.apply(&|| true),
        Err(Error::Profile(profiles::Error::BindingMismatch))
    );
    assert_eq!(operation.identity().unwrap().lifecycle_id, "lifecycle-a1");
}

#[test]
fn service_descendants_do_not_hold_receipt_open_or_get_killed() {
    let success = &fixture()["lifecycle"]["succeeded"];
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
    let mut operation = Operation::new(&client, start());
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
    let mut operation = Operation::new(&client, start());
    let original = operation.request_bytes().to_vec();
    assert_eq!(operation.apply(&|| false), Err(Error::Cancelled));
    assert!(!dir.path().join("calls").exists());
    assert_eq!(operation.apply(&|| true), Err(Error::TooLarge));
    assert_eq!(operation.request_bytes(), original);
}
