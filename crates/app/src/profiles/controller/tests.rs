use super::*;
use agent_interface_roster::profiles::operations::Status;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    root: tempfile::TempDir,
    client: Client,
    journal: PathBuf,
}
impl Fixture {
    /// A stand-in for the service's `profiles create`: it counts each call,
    /// runs `extra`, exits with `$ROOT/exit` when that is there and otherwise
    /// prints `$ROOT/reply`.
    fn new(extra: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let executable = directory.join("provider");
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$2" >> "$ROOT/calls"
printf '%s\n' "$*" >> "$ROOT/arguments"
{extra}
if [ -f "$ROOT/exit" ]; then exit "$(/bin/cat "$ROOT/exit")"; fi
/bin/cat "$ROOT/reply"
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let client = Client {
            executable,
            directory: directory.clone(),
            environment: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), directory.as_os_str().into()),
                ("ROOT".into(), directory.as_os_str().into()),
            ],
        };
        let fixture = Self {
            root,
            client,
            journal: directory.join("journal"),
        };
        fixture.made();
        fixture
    }
    fn controller(&self) -> Controller {
        Controller::new(journal::Journal::open(&self.journal).unwrap(), &self.client)
    }
    /// The service made the profile and serves it.
    fn made(&self) {
        fs::write(
            self.root.path().join("reply"),
            serde_json::to_vec(&serde_json::json!({
                "state": "serving", "protocol_version": 1, "profile_id": "0a1b2c3d4e5f",
                "display_name": "Example", "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
                "machine": {"id": "5f2c9e0d1b7a4e38"}, "socket": "/run/profile/api.sock",
                "tmux_socket": "/run/profile/tmux.sock", "credential_reference": {"type": "socket"}
            }))
            .unwrap(),
        )
        .unwrap();
    }
    fn exit(&self, code: u8) {
        fs::write(self.root.path().join("exit"), code.to_string()).unwrap();
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
    }
}
fn intent() -> Prepared {
    Prepared::create("request-a", "Example").unwrap()
}
/// Unreadable output on the first call only.
const LOST_ONCE: &str =
    "if [ ! -f \"$ROOT/once\" ]; then : > \"$ROOT/once\"; printf invalid; exit 0; fi";

#[test]
fn a_maximum_request_id_recovers_after_a_lost_reply() {
    let fixture = Fixture::new(LOST_ONCE);
    let request_id = "x".repeat(agent_interface_roster::profiles::operations::REQUEST_ID_LIMIT);
    let prepared = Prepared::create(&request_id, "Example").unwrap();
    let controller = fixture.controller();
    let saved = controller.enqueue(prepared).unwrap();
    assert!(matches!(
        controller.submit(&saved.key, &|| true),
        Err(Error::Client(_))
    ));
    drop(controller);
    let receipt = fixture.controller().refresh(&saved.key, &|| true).unwrap();
    assert_eq!(receipt.status, Status::Succeeded);
    assert_eq!(
        fixture.controller().read(&saved.key).unwrap().terminal,
        Some(receipt)
    );
    assert_eq!(fixture.calls(), "create\ncreate\n");
}

#[test]
fn intent_is_durable_before_execution_and_duplicate_ids_cannot_replace_it() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let saved = controller.enqueue(intent()).unwrap();
    assert_eq!(saved.key, Controller::key("request-a"));
    assert_eq!(saved.phase, Phase::Prepared);
    assert_eq!(
        saved.action,
        agent_interface_roster::profiles::operations::Action::Create
    );
    assert_eq!(controller.ids().unwrap(), vec![saved.key.clone()]);
    assert!(saved.identity.is_none());
    assert_eq!(controller.read(&saved.key).unwrap(), saved);
    assert_eq!(fixture.calls(), "");
    assert_eq!(
        controller.enqueue(Prepared::create("request-a", "Replacement").unwrap()),
        Err(Error::Storage(journal::Error::Conflict))
    );
    assert_eq!(
        controller.refresh(&saved.key, &|| true),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        controller.retry(&saved.key, &|| true),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        controller.submit(&saved.key, &|| false),
        Err(Error::Client(super::super::Error::Cancelled))
    );
    assert_eq!(controller.read(&saved.key).unwrap(), saved);
    assert_eq!(fixture.calls(), "");
}

#[test]
fn restart_asks_again_and_terminal_receipts_disable_all_commands() {
    let fixture = Fixture::new(LOST_ONCE);
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    assert!(matches!(
        controller.submit(&key, &|| true),
        Err(Error::Client(_))
    ));
    drop(controller);
    let restored = fixture.controller();
    assert_eq!(restored.read(&key).unwrap().phase, Phase::Submitted);
    assert_eq!(fixture.calls(), "create\n");
    assert_eq!(restored.submit(&key, &|| true), Err(Error::WrongPhase));
    let receipt = restored.refresh(&key, &|| true).unwrap();
    assert_eq!(receipt.status, Status::Succeeded);
    assert_eq!(
        fixture.controller().read(&key).unwrap().terminal,
        Some(receipt)
    );
    for result in [
        restored.submit(&key, &|| true),
        restored.retry(&key, &|| true),
        restored.refresh(&key, &|| true),
    ] {
        assert_eq!(result, Err(Error::Completed));
    }
    assert_eq!(fixture.calls(), "create\ncreate\n");
}

#[test]
fn explicit_retry_keeps_the_original_id_after_an_unknown_outcome() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    fixture.exit(4);
    assert!(controller.submit(&key, &|| true).is_err());
    // A request the service could not read is refused on any repeat.
    fixture.exit(2);
    assert_eq!(
        fixture.controller().retry(&key, &|| true).unwrap().status,
        Status::Failed
    );
    let arguments = fs::read_to_string(fixture.root.path().join("arguments")).unwrap();
    let sent: Vec<_> = arguments.lines().collect();
    assert_eq!(
        sent,
        ["profiles create --starter core-v1 --request-id request-a --json -- Example"; 2]
    );
}

#[test]
fn invalid_envelopes_fail_before_any_command() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let journal = journal::Journal::open(&fixture.journal).unwrap();
    let saved = journal.read(&key).unwrap().unwrap();
    let mut envelope: Envelope = serde_json::from_slice(saved.payload()).unwrap();
    envelope.kind = "setup".into();
    journal
        .save(
            &key,
            Some(saved.revision()),
            &serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
    assert_eq!(controller.submit(&key, &|| true), Err(Error::Invalid));
    assert_eq!(fixture.calls(), "");
}

#[test]
fn storage_failure_after_provider_success_never_returns_terminal_authority() {
    let fixture = Fixture::new("/bin/chmod 755 \"$ROOT/journal\"");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    assert_eq!(
        controller.submit(&key, &|| true),
        Err(Error::Storage(journal::Error::Unsafe))
    );
    fs::set_permissions(&fixture.journal, fs::Permissions::from_mode(0o700)).unwrap();
    let saved = fixture.controller().read(&key).unwrap();
    assert_eq!(saved.phase, Phase::Submitted);
    assert!(saved.terminal.is_none());
    assert!(saved.identity.is_none());
    assert_eq!(fixture.calls(), "create\n");
}

#[test]
fn concurrent_first_submission_is_refused_and_stale_results_cannot_overwrite_progress() {
    use std::{cell::Cell, sync::mpsc, time::Duration};
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let other = fixture.controller();
    let worker_key = key.clone();
    let (ready, notified) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let calls = Cell::new(0);
        other.submit(&worker_key, &|| {
            let current = calls.get();
            calls.set(current + 1);
            if current == 1 {
                ready.send(()).unwrap();
                return proceed.recv_timeout(Duration::from_secs(10)).is_ok();
            }
            true
        })
    });
    notified.recv_timeout(Duration::from_secs(5)).unwrap();
    let duplicate = controller.submit(&key, &|| true);
    let fresh = controller.refresh(&key, &|| true);
    release.send(()).unwrap();
    let stale = worker.join().unwrap();
    assert_eq!(duplicate, Err(Error::WrongPhase));
    let fresh = fresh.unwrap();
    assert_eq!(fresh.status, Status::Succeeded);
    assert_eq!(stale, Err(Error::Storage(journal::Error::Conflict)));
    assert_eq!(controller.read(&key).unwrap().terminal, Some(fresh));
    assert_eq!(fixture.calls(), "create\ncreate\n");
}

#[test]
fn recovery_confirms_a_terminal_record_left_by_a_post_rename_failure() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let mut stored = controller.load(&key).unwrap();
    let receipt = stored.operation.apply(&|| true).unwrap();
    let bytes = encode(Phase::Submitted, &stored.operation).unwrap();
    let journal = journal::Journal::open(&fixture.journal).unwrap();
    assert!(matches!(
        journal.save_with_sync(&key, Some(stored.record.revision()), &bytes, &|_| Err(
            journal::Error::Unavailable
        )),
        Err(journal::Error::Unavailable)
    ));
    drop(controller);
    let recovered = fixture.controller();
    assert_eq!(recovered.read(&key).unwrap().terminal, Some(receipt));
    assert_eq!(recovered.retry(&key, &|| true), Err(Error::Completed));
    assert_eq!(fixture.calls(), "create\n");
}
