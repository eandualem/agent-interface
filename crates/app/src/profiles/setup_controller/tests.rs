use super::*;
use crate::profiles::tests::{answer, cancelled, first_agent, ready, request, step};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

/// The profile requests are for, and two others.
const PROFILE: &str = "0a1b2c3d4e5f";
const OTHER: &str = "0a1b2c3d4e60";
const THIRD: &str = "0a1b2c3d4e61";
fn id() -> String {
    request(1)
}
fn intent() -> Prepared {
    intent_for(1, PROFILE)
}
fn intent_for(n: u8, profile: &str) -> Prepared {
    first_agent(&request(n), profile, "guide")
}
fn named(n: u8, name: &str) -> Prepared {
    first_agent(&request(n), PROFILE, name)
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
        let result = answer(
            &id(),
            "first_agent",
            "action_required",
            "action_needed",
            retry,
        );
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
    assert_eq!(snapshot.key, Controller::key(&id()));
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
    fixture.reply(Action::CancelFirstAgent, &cancelled(&id()), 0);
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
    // Begun already, its ID used for another request, or refused: the
    // original isn't cancelled and may be sent again.
    for (status, code, retry, exit) in [
        ("action_required", "cancellation_unsafe", "none", 3),
        ("failed", "request_conflict", "new_request", 1),
        ("failed", "invalid_request", "new_request", 2),
    ] {
        let fixture = Fixture::new("");
        fixture.needs_action("same_request");
        let controller = fixture.controller();
        let key = controller.enqueue(intent()).unwrap().key;
        controller.submit(&key, &|| true, |_| {}).unwrap();
        fixture.reply(
            Action::CancelFirstAgent,
            &answer(&id(), "cancel_first_agent", status, code, retry),
            exit,
        );
        let refused = controller.cancel(&key, &|| true, |_| {}).unwrap();
        assert!(!refused.cancelled);
        assert_eq!(
            controller.retry_cancel(&key, &|| true, |_| {}),
            Err(Error::RetryRefused)
        );
        fixture.reply(Action::FirstAgent, &ready(&id()), 0);
        let ready = fixture.controller().retry(&key, &|| true, |_| {}).unwrap();
        assert_eq!(ready.ready_agent.as_deref(), Some("guide"));
        let saved = fixture.controller().read(&key).unwrap();
        assert_eq!(saved.first_result, Some(ready));
        assert_eq!(saved.cancellation_result, Some(refused));
        assert_eq!(
            controller.cancel(&key, &|| true, |_| {}),
            Err(Error::Completed)
        );
        assert_eq!(fixture.calls(), "first\ncancel\nfirst\n");
    }
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
            Err(Error::Completed)
        );
        assert_eq!(fixture.calls(), "first\n");
        assert_eq!(controller.read(&key).unwrap().request_id, id());
    }
}

#[test]
fn foreign_checkpoint_kinds_execute_no_command() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
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
    fixture.reply(Action::FirstAgent, &ready(&id()), 0);
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
    let body = format!("{}\n{}\n", step(&id(), 1, "preflight"), ready(&id()));
    fixture.raw(Action::FirstAgent, body.as_bytes(), 0);
    fixture.reply(Action::CancelFirstAgent, &cancelled(&id()), 0);
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
        controller.enqueue(intent_for(2, PROFILE)),
        Err(Error::Pending)
    );
    fixture.raw(Action::FirstAgent, b"lost", 0);
    assert!(controller.submit(&original.key, &|| true, |_| {}).is_err());
    fixture.raw(Action::CancelFirstAgent, b"lost", 0);
    assert!(controller.cancel(&original.key, &|| true, |_| {}).is_err());
    assert_eq!(
        fixture.controller().enqueue(intent_for(2, PROFILE)),
        Err(Error::Pending)
    );
    assert_eq!(controller.ids().unwrap(), vec![original.key.clone()]);
    fixture.reply(Action::CancelFirstAgent, &cancelled(&id()), 0);
    controller
        .retry_cancel(&original.key, &|| true, |_| {})
        .unwrap();
    let next = fixture
        .controller()
        .enqueue(intent_for(2, PROFILE))
        .unwrap();
    assert_eq!(next.phase, Phase::Prepared);
    assert_eq!(next.request_id, request(2));
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
                barrier.wait();
                controller.enqueue(intent_for(10 + index as u8, PROFILE))
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
        recovered.enqueue(intent_for(6, PROFILE)),
        Err(Error::Pending)
    );
    assert_eq!(fixture.calls(), "");
}

#[test]
fn admission_blocks_the_profile_s_other_requests_and_never_ignores_invalid_records() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    assert_eq!(original.agent_name, "guide");
    assert_eq!(
        controller.enqueue(intent_for(3, PROFILE)),
        Err(Error::Pending)
    );
    // Under another name too: an open request may still make the profile's
    // first agent. Another profile is free.
    assert_eq!(controller.enqueue(named(4, "second")), Err(Error::Pending));
    controller.enqueue(intent_for(5, OTHER)).unwrap();
    assert_eq!(
        controller
            .pending(&original.profile_id, &|| true)
            .unwrap()
            .len(),
        1
    );
    let journal = journal::Journal::open(&fixture.journal).unwrap();
    journal
        .save("00000000000000000000000000000000", None, b"malformed")
        .unwrap();
    assert_eq!(
        controller.enqueue(intent_for(6, THIRD)),
        Err(Error::Invalid)
    );
    // Only the damaged record can be forgotten without a revision.
    assert_eq!(
        controller.forget(&original.key, None),
        Err(Error::WrongPhase)
    );
    controller
        .forget("00000000000000000000000000000000", None)
        .unwrap();
    controller.enqueue(intent_for(6, THIRD)).unwrap();
    assert_eq!(fixture.calls(), "");
}

#[test]
fn a_result_this_request_cannot_retry_closes_it_and_a_retryable_one_blocks_its_profile() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let result = |n: u8, code: &str, retry: &str| {
        answer(&request(n), "first_agent", "action_required", code, retry)
    };
    // Name taken: the same request can go no further, so it no longer blocks.
    fixture.reply(
        Action::FirstAgent,
        &result(1, "agent_conflict", "new_request"),
        3,
    );
    let taken = controller.enqueue(intent()).unwrap();
    controller.submit(&taken.key, &|| true, |_| {}).unwrap();
    let taken = controller.read(&taken.key).unwrap();
    assert!(taken.closed());
    assert_eq!(taken.first_result.unwrap().code.unwrap(), "agent_conflict");
    assert!(
        controller
            .pending(&taken.profile_id, &|| true)
            .unwrap()
            .is_empty()
    );
    // Not known yet: the request stays open, and holds the profile.
    fixture.reply(
        Action::FirstAgent,
        &result(2, "outcome_unknown", "same_request"),
        3,
    );
    let slow = controller.enqueue(intent_for(2, PROFILE)).unwrap();
    controller.submit(&slow.key, &|| true, |_| {}).unwrap();
    let slow = controller.read(&slow.key).unwrap();
    assert!(!slow.closed());
    assert_eq!(
        controller.enqueue(intent_for(7, PROFILE)),
        Err(Error::Pending)
    );
    assert_eq!(controller.enqueue(named(8, "second")), Err(Error::Pending));
    // Forgetting needs the revision shown, and frees the profile.
    assert_eq!(
        controller.forget(&slow.key, Some(slow.revision + 1)),
        Err(Error::Storage(journal::Error::Conflict))
    );
    controller.forget(&slow.key, Some(slow.revision)).unwrap();
    assert_eq!(controller.read(&slow.key), Err(Error::Missing));
    controller.enqueue(intent_for(9, PROFILE)).unwrap();
}

#[test]
fn an_open_request_can_be_read_and_retried_after_the_environment_changes() {
    let fixture = Fixture::new("");
    fixture.reply(Action::FirstAgent, &ready(&id()), 0);
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    let mut changed = fixture.client.clone();
    changed
        .environment
        .push(("TERM_SESSION_ID".into(), "new".into()));
    let other = Controller::new(journal::Journal::open(&fixture.journal).unwrap(), &changed);
    assert_eq!(other.read(&original.key).unwrap().request_id, id());
    other.submit(&original.key, &|| true, |_| {}).unwrap();
    assert!(other.read(&original.key).unwrap().closed());
    assert_eq!(fixture.calls(), "first\n");
}

#[test]
fn pending_observation_is_per_profile_and_cancellation_fails_closed() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    let foreign = controller.enqueue(intent_for(5, OTHER)).unwrap();
    assert_eq!(
        controller.pending(&original.profile_id, &|| true).unwrap(),
        vec![original.clone()]
    );
    assert_eq!(
        controller.pending(&foreign.profile_id, &|| true).unwrap(),
        vec![foreign]
    );
    assert_eq!(
        controller.pending(&original.profile_id, &|| false),
        Err(Error::Client(super::super::Error::Cancelled))
    );
    assert_eq!(fixture.calls(), "");
}

#[test]
fn a_retry_this_app_does_not_know_keeps_the_request_open_without_a_retry() {
    let fixture = Fixture::new("");
    let unknown = answer(&id(), "first_agent", "paused", "new_code", "later");
    fixture.reply(Action::FirstAgent, &unknown, 1);
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    let kept = controller.submit(&key, &|| true, |_| {}).unwrap();
    assert_eq!(
        (kept.status.as_str(), kept.retry.as_deref()),
        ("paused", Some("later"))
    );
    let saved = controller.read(&key).unwrap();
    assert!(!saved.closed());
    assert_eq!(
        controller.retry(&key, &|| true, |_| {}),
        Err(Error::RetryRefused)
    );
    // It still holds its name until it is forgotten.
    assert_eq!(
        controller.enqueue(intent_for(2, PROFILE)),
        Err(Error::Pending)
    );
    controller.forget(&key, Some(saved.revision)).unwrap();
    controller.enqueue(intent_for(2, PROFILE)).unwrap();
    assert_eq!(fixture.calls(), "first\n");
}

#[test]
fn a_request_saved_under_schema_2_holds_nothing_back_and_can_be_forgotten() {
    let fixtures: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap();
    let setup = &fixtures["setup"];
    let saved = json!({
        "checkpoint_version": 1,
        "first": setup["first_agent"].to_string(),
        "cancel": setup["cancel_first_agent"].to_string(),
    });
    let capsule = crate::profiles::recovery::checkpoint(
        crate::profiles::recovery::Kind::Setup,
        &serde_json::to_vec(&saved).unwrap(),
    )
    .unwrap();
    let envelope = Envelope {
        format: 1,
        kind: "setup".into(),
        phase: Phase::Prepared,
        capsule: String::from_utf8(capsule).unwrap(),
        first: None,
        cancel: None,
    };
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = Controller::key(setup["first_agent"]["request_id"].as_str().unwrap());
    journal::Journal::open(&fixture.journal)
        .unwrap()
        .save(&key, None, &serde_json::to_vec(&envelope).unwrap())
        .unwrap();
    assert_eq!(controller.read(&key), Err(Error::Earlier));
    // It holds back no other request, of this profile or another, and stays
    // listed until it is forgotten.
    assert_eq!(controller.pending(PROFILE, &|| true), Ok(Vec::new()));
    let added = controller.enqueue(intent()).unwrap();
    controller.enqueue(intent_for(2, OTHER)).unwrap();
    assert_eq!(controller.pending(PROFILE, &|| true).unwrap(), vec![added]);
    assert_eq!(controller.ids().unwrap().len(), 3);
    controller.forget(&key, None).unwrap();
    assert_eq!(controller.read(&key), Err(Error::Missing));
    assert_eq!(fixture.calls(), "");
}

#[test]
fn an_expired_cancellation_releases_the_profile_and_keeps_what_is_unknown() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let original = controller.enqueue(intent()).unwrap();
    fixture.raw(Action::FirstAgent, b"lost", 0);
    assert!(controller.submit(&original.key, &|| true, |_| {}).is_err());
    fixture.reply(
        Action::CancelFirstAgent,
        &answer(
            &id(),
            "cancel_first_agent",
            "failed",
            "request_expired",
            "none",
        ),
        1,
    );
    let expired = controller.cancel(&original.key, &|| true, |_| {}).unwrap();
    assert!(!expired.cancelled);
    // It never runs now: the profile takes a new request, and this one keeps
    // its unknown outcome and its history until it is forgotten.
    let saved = controller.read(&original.key).unwrap();
    assert!(saved.closed() && saved.first_result.is_none());
    assert_eq!(saved.cancellation_result, Some(expired));
    assert_eq!(
        controller.retry(&original.key, &|| true, |_| {}),
        Err(Error::Completed)
    );
    controller.enqueue(intent_for(2, PROFILE)).unwrap();
    assert_eq!(fixture.calls(), "first\ncancel\n");
}

#[test]
fn an_outcome_of_an_earlier_schema_under_this_request_fails_closed() {
    let fixture = Fixture::new("");
    let controller = fixture.controller();
    let key = controller.enqueue(intent()).unwrap().key;
    fixture.reply(Action::FirstAgent, &ready(&id()), 0);
    controller.submit(&key, &|| true, |_| {}).unwrap();
    let journal = journal::Journal::open(&fixture.journal).unwrap();
    let record = journal.read(&key).unwrap().unwrap();
    let mut envelope: Envelope = serde_json::from_slice(record.payload()).unwrap();
    let first = envelope.first.as_ref().unwrap();
    let mut outcome: Value = serde_json::from_str(first).unwrap();
    let mut result: Value = serde_json::from_str(outcome["result"].as_str().unwrap()).unwrap();
    result["schema_version"] = 2.into();
    outcome["result"] = result.to_string().into();
    envelope.first = Some(outcome.to_string());
    journal
        .save(
            &key,
            Some(record.revision()),
            &serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
    // Only the request's own schema makes a record an earlier one's.
    assert_eq!(controller.read(&key), Err(Error::Invalid));
}
