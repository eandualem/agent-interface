use super::*;
use agent_interface_roster::profiles::{
    Profile,
    setup::{Agent, Brief},
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

fn fixtures() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn intent() -> Prepared {
    intent_for("setup-a1", "lifecycle-a1")
}
fn intent_for(request_id: &str, lifecycle: &str) -> Prepared {
    let mut wire = fixtures()["discovery"]["show_ready"].clone();
    wire["binding"]["lifecycle_id"] = lifecycle.into();
    let profile = Profile::parse(&serde_json::to_vec(&wire).unwrap()).unwrap();
    Prepared::first_agent(
        request_id,
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
struct Fixture {
    root: tempfile::TempDir,
    journal: PathBuf,
    client: Client,
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
case "$(/bin/cat "$3")" in
  *'"action":"cancel_first_agent"'*) action=cancel;;
  *) action=first;;
esac
printf '%s\n' "$action" >> "$ROOT/calls"
/bin/cat "$3" >> "$ROOT/bodies"; printf '\n' >> "$ROOT/bodies"
{extra}
/bin/cat "$ROOT/$action.reply"
code=$(/bin/cat "$ROOT/$action.exit")
exit "$code"
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
            journal: directory.join("journal"),
            client,
        }
    }
    fn controller(&self) -> Controller {
        Controller::new(journal::Journal::open(&self.journal).unwrap(), &self.client)
    }
    fn reply(&self, action: Action, value: &Value, exit: u8) {
        self.raw(action, &serde_json::to_vec(value).unwrap(), exit);
    }
    fn raw(&self, action: Action, bytes: &[u8], exit: u8) {
        let name = match action {
            Action::FirstAgent => "first",
            Action::CancelFirstAgent => "cancel",
        };
        fs::write(self.root.path().join(format!("{name}.reply")), bytes).unwrap();
        fs::write(
            self.root.path().join(format!("{name}.exit")),
            exit.to_string(),
        )
        .unwrap();
    }
    fn needs_action(&self, retry: &str) {
        let mut result = fixtures()["setup"]["result"].clone();
        result["status"] = "action_required".into();
        result["code"] = "action_needed".into();
        result["retry"] = retry.into();
        result["data"] = json!({});
        self.reply(Action::FirstAgent, &result, 3);
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
    }
}

#[test]
fn saved_intent_executes_nothing_and_first_submission_is_claimed_once() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let snapshot = controller.enqueue(intent()).unwrap();
    assert_eq!(controller.ids().unwrap(), vec![snapshot.key.clone()]);
    assert_eq!(snapshot.phase, Phase::Prepared);
    assert_eq!(snapshot.key, Controller::key("setup-a1"));
    assert_eq!(controller.read(&snapshot.key).unwrap(), snapshot);
    assert_eq!(
        controller.enqueue(intent()),
        Err(Error::Storage(journal::Error::Conflict))
    );
    assert_eq!(
        controller.retry(&snapshot.key, &|| true, |_| {}),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        controller.cancel(&snapshot.key, &|| true, |_| {}),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        controller.submit(&snapshot.key, &|| false, |_| {}),
        Err(Error::Client(super::super::Error::Cancelled))
    );
    assert_eq!(fixture.calls(), "");
    assert_eq!(controller.read(&snapshot.key).unwrap(), snapshot);
    fixture.needs_action("same_request");
    assert_eq!(
        controller
            .submit(&snapshot.key, &|| true, |_| {})
            .unwrap()
            .status,
        "action_required"
    );
    assert_eq!(
        controller.submit(&snapshot.key, &|| true, |_| {}),
        Err(Error::WrongPhase)
    );
    assert_eq!(fixture.calls(), "first\n");
}

#[test]
fn lost_cancellation_reply_can_only_retry_the_original_cancel_after_restart() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    fixture.raw(Action::FirstAgent, b"unknown", 0);
    assert!(controller.submit(&key, &|| true, |_| {}).is_err());
    fixture.raw(Action::CancelFirstAgent, b"lost", 0);
    assert!(controller.cancel(&key, &|| true, |_| {}).is_err());
    drop(controller);
    let recovered = fixture.controller();
    let snapshot = recovered.read(&key).unwrap();
    assert_eq!(snapshot.phase, Phase::CancelPending);
    assert!(snapshot.cancellation_result.is_none());
    assert_eq!(fixture.calls(), "first\ncancel\n");
    assert_eq!(
        recovered.retry(&key, &|| true, |_| {}),
        Err(Error::WrongPhase)
    );
    assert_eq!(
        recovered.cancel(&key, &|| true, |_| {}),
        Err(Error::WrongPhase)
    );
    fixture.reply(
        Action::CancelFirstAgent,
        &fixtures()["setup"]["cancellation_result"],
        0,
    );
    assert!(
        recovered
            .retry_cancel(&key, &|| true, |_| {})
            .unwrap()
            .cancelled
    );
    let snapshot = fixture.controller().read(&key).unwrap();
    assert!(snapshot.cancellation_result.unwrap().cancelled);
    assert_eq!(
        recovered.retry(&key, &|| true, |_| {}),
        Err(Error::Completed)
    );
    assert_eq!(
        recovered.retry_cancel(&key, &|| true, |_| {}),
        Err(Error::Completed)
    );
    let bodies = fs::read_to_string(fixture.root.path().join("bodies")).unwrap();
    let lines: Vec<_> = bodies.lines().collect();
    let prepared = intent();
    assert_eq!(lines[0].as_bytes(), prepared.bytes(Action::FirstAgent));
    assert_eq!(
        lines[1].as_bytes(),
        prepared.bytes(Action::CancelFirstAgent)
    );
    assert_eq!(lines[1], lines[2]);
    assert_eq!(fixture.calls(), "first\ncancel\ncancel\n");
}

#[test]
fn unsafe_cancellation_preserves_original_setup_and_respects_both_retry_policies() {
    let fixture = Fixture::new("");
    fixture.needs_action("same_request");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    controller.submit(&key, &|| true, |_| {}).unwrap();
    fixture.reply(
        Action::CancelFirstAgent,
        &fixtures()["setup"]["cancellation_unsafe"],
        3,
    );
    let refused = controller.cancel(&key, &|| true, |_| {}).unwrap();
    assert!(!refused.cancelled);
    assert_eq!(
        controller.retry_cancel(&key, &|| true, |_| {}),
        Err(Error::RetryRefused)
    );
    fixture.reply(Action::FirstAgent, &fixtures()["setup"]["result"], 0);
    let ready = fixture.controller().retry(&key, &|| true, |_| {}).unwrap();
    assert_eq!(ready.ready_agent.as_deref(), Some("worker"));
    let saved = fixture.controller().read(&key).unwrap();
    assert_eq!(saved.first_result, Some(ready));
    assert_eq!(saved.cancellation_result, Some(refused));
    assert_eq!(
        controller.cancel(&key, &|| true, |_| {}),
        Err(Error::Completed)
    );
    assert_eq!(fixture.calls(), "first\ncancel\nfirst\n");
}

#[test]
fn new_request_and_none_policies_never_implicitly_retry_or_allocate() {
    for policy in ["new_request", "none"] {
        let fixture = Fixture::new("");
        fixture.needs_action(policy);
        let controller = fixture.controller();
        let key = controller.enqueue(intent()).unwrap().key;
        controller.submit(&key, &|| true, |_| {}).unwrap();
        assert_eq!(
            fixture.controller().retry(&key, &|| true, |_| {}),
            Err(Error::RetryRefused)
        );
        assert_eq!(fixture.calls(), "first\n");
        assert_eq!(controller.read(&key).unwrap().request_id, "setup-a1");
    }
}

#[test]
fn changed_context_and_foreign_checkpoint_kinds_execute_no_command() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let mut changed = fixture.client.clone();
    changed
        .environment
        .push(("BACKBONE_AGENT".into(), "changed".into()));
    let other = Controller::new(journal::Journal::open(&fixture.journal).unwrap(), &changed);
    assert_eq!(
        other.submit(&key, &|| true, |_| {}),
        Err(Error::Client(super::super::Error::ContextChanged))
    );
    let journal = journal::Journal::open(&fixture.journal).unwrap();
    let record = journal.read(&key).unwrap().unwrap();
    let mut envelope: Envelope = serde_json::from_slice(record.payload()).unwrap();
    envelope.kind = "lifecycle".into();
    journal
        .save(
            &key,
            Some(record.revision()),
            &serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
    assert_eq!(controller.read(&key), Err(Error::Invalid));
    assert_eq!(fixture.calls(), "");
}

#[test]
fn a_storage_failure_withholds_readiness_even_after_matching_process_exit() {
    let fixture = Fixture::new("/bin/chmod 755 \"$ROOT/journal\"");
    fixture.reply(Action::FirstAgent, &fixtures()["setup"]["result"], 0);
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    assert_eq!(
        controller.submit(&key, &|| true, |_| {}),
        Err(Error::Storage(journal::Error::Unsafe))
    );
    fs::set_permissions(&fixture.journal, fs::Permissions::from_mode(0o700)).unwrap();
    let snapshot = fixture.controller().read(&key).unwrap();
    assert_eq!(snapshot.phase, Phase::Submitted);
    assert!(snapshot.first_result.is_none());
    assert_eq!(fixture.calls(), "first\n");
}

#[test]
fn progress_is_stamped_and_a_cancel_winner_discards_stale_first_agent_success() {
    use std::{sync::mpsc, time::Duration};
    let fixture = Fixture::new("");
    let f = fixtures();
    let body = format!("{}\n{}\n", f["setup"]["progress"], f["setup"]["result"]);
    fixture.raw(Action::FirstAgent, body.as_bytes(), 0);
    fixture.reply(
        Action::CancelFirstAgent,
        &f["setup"]["cancellation_result"],
        0,
    );
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let other = fixture.controller();
    let worker_key = key.clone();
    let (ready, notified) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        other.submit(&worker_key, &|| true, |progress| {
            ready.send(progress).unwrap();
            proceed.recv_timeout(Duration::from_secs(10)).unwrap();
        })
    });
    let progress = notified.recv_timeout(Duration::from_secs(5)).unwrap();
    let duplicate = controller.submit(&key, &|| true, |_| {});
    let cancelled = controller.cancel(&key, &|| true, |_| {});
    release.send(()).unwrap();
    let stale = worker.join().unwrap();
    assert_eq!(duplicate, Err(Error::WrongPhase));
    assert!(cancelled.unwrap().cancelled);
    assert_eq!(stale, Err(Error::Storage(journal::Error::Conflict)));
    let saved = controller.read(&key).unwrap();
    assert!(saved.first_result.is_none());
    assert!(saved.cancellation_result.unwrap().cancelled);
    assert_eq!(progress.key, key);
    assert!(progress.revision < saved.revision);
    assert!(progress.event.step.is_some());
    assert!(progress.event.ready_agent.is_none());
    assert_eq!(fixture.calls(), "first\ncancel\n");
}

#[test]
fn new_choices_wait_for_durable_safe_cancellation_of_the_original() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    assert_eq!(
        controller.enqueue(intent_for("setup-a2", "lifecycle-a1")),
        Err(Error::Pending)
    );
    fixture.raw(Action::FirstAgent, b"lost", 0);
    assert!(controller.submit(&original.key, &|| true, |_| {}).is_err());
    fixture.raw(Action::CancelFirstAgent, b"lost", 0);
    assert!(controller.cancel(&original.key, &|| true, |_| {}).is_err());
    assert_eq!(
        fixture
            .controller()
            .enqueue(intent_for("setup-a2", "lifecycle-a1")),
        Err(Error::Pending)
    );
    assert_eq!(controller.ids().unwrap(), vec![original.key.clone()]);
    fixture.reply(
        Action::CancelFirstAgent,
        &fixtures()["setup"]["cancellation_result"],
        0,
    );
    controller
        .retry_cancel(&original.key, &|| true, |_| {})
        .unwrap();
    let next = fixture
        .controller()
        .enqueue(intent_for("setup-a2", "lifecycle-a1"))
        .unwrap();
    assert_eq!(next.phase, Phase::Prepared);
    assert_eq!(next.request_id, "setup-a2");
    assert_eq!(controller.ids().unwrap().len(), 2);
    assert!(
        controller
            .read(&original.key)
            .unwrap()
            .cancellation_result
            .unwrap()
            .cancelled
    );
    assert_eq!(fixture.calls(), "first\ncancel\ncancel\n");
}

#[test]
fn concurrent_fresh_choices_admit_only_one_request_for_the_instance() {
    use std::sync::{Arc, Barrier};
    let fixture = Fixture::new("");
    let controllers = [fixture.controller(), fixture.controller()];
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = controllers
        .into_iter()
        .enumerate()
        .map(|(index, controller)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let request = format!("setup-{index}");
                barrier.wait();
                controller.enqueue(intent_for(&request, "lifecycle-a1"))
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results.iter().any(|r| matches!(
            r,
            Err(Error::Pending | Error::Storage(journal::Error::Busy))
        )),
        "{results:?}"
    );
    let recovered = fixture.controller();
    assert_eq!(recovered.ids().unwrap().len(), 1);
    assert_eq!(
        recovered.enqueue(intent_for("another", "lifecycle-a1")),
        Err(Error::Pending)
    );
    assert_eq!(fixture.calls(), "");
}

#[test]
fn admission_inspects_other_contexts_but_never_ignores_invalid_records() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    controller
        .enqueue(intent_for("other-life", "lifecycle-a2"))
        .unwrap();
    let mut changed = fixture.client.clone();
    changed
        .environment
        .push(("CONTEXT_VERSION".into(), "changed".into()));
    let foreign = Controller::new(journal::Journal::open(&fixture.journal).unwrap(), &changed);
    assert_eq!(
        foreign.enqueue(intent_for("same-instance", "lifecycle-a1")),
        Err(Error::Pending)
    );
    assert_eq!(
        foreign.pending(&original.identity, &|| true).unwrap(),
        vec![original]
    );
    foreign
        .enqueue(intent_for("other-caller", "lifecycle-a3"))
        .unwrap();
    let journal = journal::Journal::open(&fixture.journal).unwrap();
    journal
        .save("00000000000000000000000000000000", None, b"malformed")
        .unwrap();
    assert_eq!(
        controller.enqueue(intent_for("another", "lifecycle-a4")),
        Err(Error::Invalid)
    );
    assert_eq!(controller.ids().unwrap().len(), 4);
    assert_eq!(fixture.calls(), "");
}

#[test]
fn closed_format_one_records_do_not_block_admission_after_context_change() {
    let fixture = Fixture::new("");
    fixture.reply(Action::FirstAgent, &fixtures()["setup"]["result"], 0);
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    controller.submit(&original.key, &|| true, |_| {}).unwrap();
    let mut changed = fixture.client.clone();
    changed
        .environment
        .push(("SESSION_VERSION".into(), "new".into()));
    let other = Controller::new(journal::Journal::open(&fixture.journal).unwrap(), &changed);
    assert!(
        other
            .pending(&original.identity, &|| true)
            .unwrap()
            .is_empty()
    );
    other
        .enqueue(intent_for("setup-a2", "lifecycle-a1"))
        .unwrap();
    assert_eq!(
        other.read(&original.key),
        Err(Error::Client(super::super::Error::ContextChanged))
    );
    assert_eq!(
        other.retry(&original.key, &|| true, |_| {}),
        Err(Error::Client(super::super::Error::ContextChanged))
    );
    assert_eq!(fixture.calls(), "first\n");
}

#[test]
fn pending_observation_retains_full_origin_and_cancellation_fails_closed() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    let foreign = controller
        .enqueue(intent_for("other-life", "lifecycle-a2"))
        .unwrap();
    assert_eq!(
        controller.pending(&original.identity, &|| true).unwrap(),
        vec![original.clone()]
    );
    assert_eq!(
        controller.pending(&foreign.identity, &|| true).unwrap(),
        vec![foreign]
    );
    assert_eq!(
        controller.pending(&original.identity, &|| false),
        Err(Error::Client(super::super::Error::Cancelled))
    );
    assert_eq!(fixture.calls(), "");
}
