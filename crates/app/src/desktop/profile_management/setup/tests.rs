use super::*;
use agent_interface_roster::profiles::{Identity, setup::Action};

fn snapshot() -> Snapshot {
    Snapshot {
        key: "key-a".into(),
        revision: 7,
        phase: Phase::Submitted,
        identity: Identity {
            profile_id: "profile-a".into(),
            installation_id: "install-a".into(),
            lifecycle_id: "life-a".into(),
        },
        request_id: "request-a".into(),
        first_result: None,
        cancellation_result: None,
    }
}
fn event(s: &Snapshot) -> Event {
    Event {
        identity: s.identity.clone(),
        request_id: s.request_id.clone(),
        action: Action::FirstAgent,
        sequence: 1,
        step: Some("launch".into()),
        status: "started".into(),
        code: None,
        retry: None,
        cancelled: false,
        ready_agent: None,
        launch_id: None,
    }
}
#[test]
fn unknown_cancel_never_offers_setup_or_a_new_cancel() {
    let mut s = snapshot();
    s.phase = Phase::CancelPending;
    assert!(!Command::Submit.allowed(Some(&s)));
    assert!(!Command::Retry.allowed(Some(&s)));
    assert!(!Command::Cancel.allowed(Some(&s)));
    assert!(Command::RetryCancel.allowed(Some(&s)));
    let mut cancellation = event(&s);
    cancellation.action = Action::CancelFirstAgent;
    cancellation.status = "action_required".into();
    cancellation.code = Some("cancellation_unsafe".into());
    cancellation.retry = Some("same_request".into());
    s.cancellation_result = Some(cancellation);
    assert!(Command::Retry.allowed(Some(&s)));
    s.cancellation_result.as_mut().unwrap().cancelled = true;
    for command in [
        Command::Submit,
        Command::Retry,
        Command::Cancel,
        Command::RetryCancel,
    ] {
        assert!(!command.allowed(Some(&s)));
    }
    assert!(Command::Read.allowed(Some(&s)));
}
#[test]
fn progress_requires_original_key_tuple_request_and_newer_revision() {
    let s = snapshot();
    let mut p = Progress {
        key: s.key.clone(),
        revision: 8,
        event: event(&s),
    };
    assert!(progress_matches(Some(&s), None, &s.key, &p));
    assert!(!progress_matches(Some(&s), Some(&p), &s.key, &p));
    p.event.identity.lifecycle_id = "life-b".into();
    assert!(!progress_matches(Some(&s), None, &s.key, &p));
    p.event = event(&s);
    p.revision = s.revision;
    assert!(!progress_matches(Some(&s), None, &s.key, &p));
    p.revision = 8;
    p.key = "other-key".into();
    assert!(!progress_matches(Some(&s), None, &s.key, &p));
    p.key = s.key.clone();
    p.event.request_id = "other-request".into();
    assert!(!progress_matches(Some(&s), None, &s.key, &p));
}
#[test]
fn readonly_reload_remains_available_when_record_cannot_be_restored() {
    assert!(Command::Read.allowed(None));
    for command in [
        Command::Submit,
        Command::Retry,
        Command::Cancel,
        Command::RetryCancel,
    ] {
        assert!(!command.allowed(None));
    }
    let mut s = snapshot();
    let mut failure = event(&s);
    failure.retry = Some("new_request".into());
    s.first_result = Some(failure);
    assert!(!Command::Retry.allowed(Some(&s)));
    assert!(Command::Cancel.allowed(Some(&s)));
}

#[test]
fn bounded_history_keeps_invalid_records_visible_and_never_invokes_provider() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let path = directory.join("setup");
    let journal = journal::Journal::open(&path).unwrap();
    for index in 0..(PAGE_SIZE + 1) {
        journal
            .save(&format!("{index:032x}"), None, b"invalid envelope")
            .unwrap();
    }
    let client = Client::new(directory.join("absent-provider"), "/usr/bin:/bin".into()).unwrap();
    let controller = Controller::new(journal, &client);
    let live = AtomicBool::new(true);
    let Page::History { ids, offset, rows } = history(&controller, None, 0, &live).unwrap() else {
        panic!("history expected")
    };
    assert_eq!(offset, 0);
    assert_eq!(ids.len(), PAGE_SIZE + 1);
    assert_eq!(rows.len(), PAGE_SIZE);
    assert!(
        rows.iter()
            .all(|(_, result)| *result == Err(Error::Invalid))
    );
    let Page::History { rows, .. } = history(&controller, Some(ids), PAGE_SIZE, &live).unwrap()
    else {
        panic!("history expected")
    };
    assert_eq!(rows.len(), 1);
    live.store(false, Ordering::Release);
    assert!(matches!(
        history(&controller, None, 0, &live),
        Err(Error::Client(crate::profiles::Error::Cancelled))
    ));
}
