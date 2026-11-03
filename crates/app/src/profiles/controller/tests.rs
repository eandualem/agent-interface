use super::*;
use agent_interface_roster::profiles::operations::Status;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    root: tempfile::TempDir,
    client: Client,
    journal: PathBuf,
}
impl Fixture {
    fn new(extra: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let executable = directory.join("provider");
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$2" >> "$ROOT/calls"
if [ "$2" = apply ]; then
    /bin/cat "$4" >> "$ROOT/bodies"
    printf '\n' >> "$ROOT/bodies"
fi
{extra}
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
        Self {
            root,
            client,
            journal: directory.join("journal"),
        }
    }
    fn controller(&self) -> Controller {
        Controller::new(journal::Journal::open(&self.journal).unwrap(), &self.client)
    }
    fn reply(&self, status: &str, lifecycle: &str) {
        fs::write(self.root.path().join("reply"), serde_json::to_vec(&serde_json::json!({
            "schema_version":1,"request_id":"request-a","action":"create","profile_id":"profile-a",
            "installation_id":"installation-a","lifecycle_id":lifecycle,"status":status,
            "error": if status == "failed" { serde_json::json!({"code":"failed","message":"Fixture failure"}) } else { serde_json::Value::Null }
        })).unwrap()).unwrap();
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
    }
}
fn intent() -> Prepared {
    Prepared::create("request-a", "Example", "baseline-a").unwrap()
}

#[test]
fn maximum_lifecycle_id_and_large_escaped_receipt_recover_after_lost_reply() {
    let fixture = Fixture::new("if [ \"$2\" = apply ]; then printf invalid; exit 0; fi");
    let request_id = "x".repeat(agent_interface_roster::profiles::operations::REQUEST_ID_LIMIT);
    let escaped = "\\".repeat((super::super::OUTPUT_LIMIT - 4096) / 2);
    let prepared = Prepared::create(&request_id, &escaped, "baseline-a").unwrap();
    let reply = serde_json::to_vec(&serde_json::json!({
        "schema_version":1,"request_id":request_id,"action":"create",
        "profile_id":escaped,"installation_id":"installation-a",
        "lifecycle_id":"lifecycle-a","status":"succeeded","error":null,
    }))
    .unwrap();
    assert!(prepared.bytes().len() <= super::super::OUTPUT_LIMIT);
    assert!(reply.len() <= super::super::OUTPUT_LIMIT);
    fs::write(fixture.root.path().join("reply"), reply).unwrap();
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
    assert_eq!(fixture.calls(), "apply\noperation\n");
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
        controller.enqueue(Prepared::create("request-a", "Replacement", "other").unwrap()),
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
fn restart_defaults_to_lookup_and_terminal_receipts_disable_all_commands() {
    let fixture = Fixture::new("if [ \"$2\" = apply ]; then printf invalid; exit 0; fi");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    assert!(matches!(
        controller.submit(&key, &|| true),
        Err(Error::Client(_))
    ));
    drop(controller);
    let restored = fixture.controller();
    assert_eq!(restored.read(&key).unwrap().phase, Phase::Submitted);
    assert_eq!(fixture.calls(), "apply\n");
    assert_eq!(restored.submit(&key, &|| true), Err(Error::WrongPhase));
    fixture.reply("succeeded", "lifecycle-a1");
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
    assert_eq!(fixture.calls(), "apply\noperation\n");
}

#[test]
fn explicit_retry_retains_exact_bytes_and_original_id_after_unknown_outcome() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = intent().bytes().to_vec();
    let key = controller.enqueue(intent()).unwrap().key;
    fs::write(fixture.root.path().join("reply"), b"unknown").unwrap();
    assert!(controller.submit(&key, &|| true).is_err());
    fixture.reply("failed", "lifecycle-a1");
    assert_eq!(
        fixture.controller().retry(&key, &|| true).unwrap().status,
        Status::Failed
    );
    let body = String::from_utf8(original).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("bodies")).unwrap(),
        format!("{body}\n{body}\n")
    );
    assert_eq!(fixture.calls(), "apply\napply\n");
}

#[test]
fn reservations_survive_restart_and_reject_a_replacement_receipt() {
    let fixture = Fixture::new("");
    fixture.reply("pending", "lifecycle-a1");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    assert_eq!(
        controller.submit(&key, &|| true).unwrap().status,
        Status::Pending
    );
    fixture.reply("succeeded", "replacement");
    let restored = fixture.controller();
    assert_eq!(
        restored.refresh(&key, &|| true),
        Err(Error::Client(super::super::Error::Profile(
            agent_interface_roster::profiles::Error::BindingMismatch
        )))
    );
    let saved = restored.read(&key).unwrap();
    assert_eq!(saved.identity.unwrap().lifecycle_id, "lifecycle-a1");
    assert!(saved.terminal.is_none());
    fixture.reply("succeeded", "lifecycle-a1");
    assert_eq!(
        restored.refresh(&key, &|| true).unwrap().status,
        Status::Succeeded
    );
}

#[test]
fn caller_changes_and_invalid_envelopes_fail_before_any_command() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let mut changed = fixture.client.clone();
    changed
        .environment
        .push(("BACKBONE_AGENT".into(), "changed".into()));
    let other = Controller::new(journal::Journal::open(&fixture.journal).unwrap(), &changed);
    assert_eq!(
        other.submit(&key, &|| true),
        Err(Error::Client(super::super::Error::ContextChanged))
    );
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
    fixture.reply("succeeded", "lifecycle-a1");
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
    assert_eq!(fixture.calls(), "apply\n");
}

#[test]
fn concurrent_first_submission_is_refused_and_stale_results_cannot_overwrite_progress() {
    use std::{cell::Cell, sync::mpsc, time::Duration};
    let fixture = Fixture::new("");
    fixture.reply("pending", "lifecycle-a1");
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
    assert_eq!(fresh.unwrap().status, Status::Pending);
    assert_eq!(stale, Err(Error::Storage(journal::Error::Conflict)));
    let saved = controller.read(&key).unwrap();
    assert_eq!(saved.identity.unwrap().lifecycle_id, "lifecycle-a1");
    assert!(saved.terminal.is_none());
    assert_eq!(fixture.calls(), "operation\napply\n");
}

#[test]
fn recovery_confirms_a_terminal_record_left_by_a_post_rename_failure() {
    let fixture = Fixture::new("");
    fixture.reply("succeeded", "lifecycle-a1");
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
    assert_eq!(fixture.calls(), "apply\n");
}

#[test]
fn interrupted_adoption_recovers_original_preflight_bytes_after_a_fresh_preview() {
    use agent_interface_roster::profiles::operations::{Action, AdoptionPreview};
    let fixture = Fixture::new(
        "if [ \"$2\" = apply ] && [ ! -f \"$ROOT/attempted\" ]; then /usr/bin/touch \"$ROOT/attempted\"; printf invalid; exit 0; fi",
    );
    let preview = |token: &str| {
        AdoptionPreview::parse(serde_json::json!({
        "schema_version":1,"installation_id":"installation-a","data_dir":"/not-opened/data",
        "tmux":{"socket_path":"/not-opened/tmux.sock"},"baseline_version":"baseline-a","adoption_token":token
    }).to_string().as_bytes()).unwrap()
    };
    let prepared = Prepared::adopt_default("request-a", &preview("first-token")).unwrap();
    let original = prepared.bytes().to_vec();
    let controller = fixture.controller();
    let saved = controller.enqueue(prepared).unwrap();
    assert!(controller.submit(&saved.key, &|| true).is_err());
    drop(controller);
    let later = Prepared::adopt_default("another-request", &preview("later-token")).unwrap();
    assert_ne!(later.bytes(), original);
    fs::write(fixture.root.path().join("reply"), serde_json::json!({
        "schema_version":1,"request_id":"request-a","action":"adopt_default","profile_id":"profile-a",
        "installation_id":"installation-a","lifecycle_id":"lifecycle-a","status":"succeeded","error":null
    }).to_string()).unwrap();
    let restored = fixture.controller();
    let snapshot = restored.read(&saved.key).unwrap();
    assert_eq!(snapshot.action, Action::AdoptDefault);
    assert_eq!(snapshot.phase, Phase::Submitted);
    assert_eq!(
        restored.submit(&saved.key, &|| true),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        restored.retry(&saved.key, &|| true).unwrap().action,
        Action::AdoptDefault
    );
    let bodies = fs::read_to_string(fixture.root.path().join("bodies")).unwrap();
    assert_eq!(bodies.lines().count(), 2);
    assert!(bodies.lines().all(|body| body.as_bytes() == original));
    assert!(restored.read(&saved.key).unwrap().terminal.is_some());
}
