use super::*;
use agent_interface_roster::profiles::operations::Receipt;

#[test]
fn superseded_and_closed_controls_cannot_accept_delayed_results() {
    let mut owner = Ticket::default();
    let a = owner.begin();
    let b = owner.begin();
    assert!(!a.load(Ordering::Acquire));
    assert!(!owner.accepts(&a));
    assert!(owner.accepts(&b));
    owner.cancel();
    assert!(!owner.accepts(&b));
    let reopened = owner.begin();
    assert!(owner.accepts(&reopened));
    assert!(!owner.accepts(&b));
    drop(owner);
    assert!(!reopened.load(Ordering::Acquire));
}

#[test]
fn unknown_storage_and_submission_never_enable_replacement_submission() {
    assert!(Command::Read.allowed(None));
    for command in [Command::Submit, Command::Refresh, Command::Retry] {
        assert!(!command.allowed(None));
    }
    let mut snapshot = Snapshot {
        key: "key".into(),
        revision: 1,
        phase: Phase::Prepared,
        action: Action::Start,
        request_id: "request-a".into(),
        identity: None,
        terminal: None,
    };
    assert!(Command::Submit.allowed(Some(&snapshot)));
    assert!(!Command::Refresh.allowed(Some(&snapshot)));
    assert!(!Command::Retry.allowed(Some(&snapshot)));
    snapshot.phase = Phase::Submitted;
    assert!(!Command::Submit.allowed(Some(&snapshot)));
    assert!(Command::Refresh.allowed(Some(&snapshot)));
    assert!(Command::Retry.allowed(Some(&snapshot)));
    snapshot.terminal = Some(Receipt {
        request_id: snapshot.request_id.clone(),
        action: snapshot.action,
        identity: None,
        status: Status::Failed,
        error_code: Some("failed".into()),
    });
    for command in [Command::Submit, Command::Refresh, Command::Retry] {
        assert!(!command.allowed(Some(&snapshot)));
    }
    assert!(Command::Read.allowed(Some(&snapshot)));
}

#[test]
fn history_reads_one_page_and_preserves_individual_invalid_record_failures() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    // Captured context is retained, but this test never invokes a command.
    let client = Client::new(directory.join("never-executed"), "/usr/bin:/bin".into()).unwrap();
    let controller = Controller::new(
        journal::Journal::open(&directory.join("journal")).unwrap(),
        &client,
    );
    for i in 0..35 {
        controller
            .enqueue(Prepared::create(&format!("request-{i}"), "Example", "baseline").unwrap())
            .unwrap();
    }
    let ids = controller.ids().unwrap();
    assert_eq!(ids.len(), 35);
    let storage = journal::Journal::open(&directory.join("journal")).unwrap();
    let record = storage.read(&ids[17]).unwrap().unwrap();
    storage
        .save(&ids[17], Some(record.revision()), b"invalid capsule")
        .unwrap();
    let live = AtomicBool::new(true);
    let Page::History {
        ids: first_ids,
        rows,
        offset,
    } = history(&controller, None, 0, &live).unwrap()
    else {
        panic!("history");
    };
    assert_eq!(offset, 0);
    assert_eq!(rows.len(), PAGE_SIZE);
    assert!(rows.iter().all(|(_, result)| result.is_ok()));
    let Page::History { rows, offset, .. } =
        history(&controller, Some(first_ids.clone()), PAGE_SIZE, &live).unwrap()
    else {
        panic!("history");
    };
    assert_eq!(offset, PAGE_SIZE);
    assert_eq!(rows.len(), PAGE_SIZE);
    assert_eq!(rows[1].0, ids[17]);
    assert_eq!(rows[1].1, Err(controller::Error::Invalid));
    assert!(rows[0].1.is_ok());
    let Page::History { rows, offset, .. } =
        history(&controller, Some(first_ids), usize::MAX, &live).unwrap()
    else {
        panic!("history");
    };
    assert_eq!(offset, 32);
    assert_eq!(rows.len(), 3);
    live.store(false, Ordering::Release);
    assert!(matches!(
        history(&controller, None, 0, &live),
        Err(controller::Error::Client(crate::profiles::Error::Cancelled))
    ));
}

#[test]
fn unsafe_lifecycle_storage_does_not_hide_independent_setup_recovery() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let lifecycle = directory.join("profile-operations");
    fs::create_dir(&lifecycle).unwrap();
    fs::set_permissions(&lifecycle, fs::Permissions::from_mode(0o755)).unwrap();
    let client = Client::new(directory.join("absent-provider"), "/usr/bin:/bin".into()).unwrap();
    let opened = open_management(
        client,
        Some(directory.clone()),
        None,
        &AtomicBool::new(true),
    )
    .unwrap();
    assert_eq!(opened.directory, directory);
    assert!(opened.controller.is_none());
    assert!(matches!(
        opened.page,
        Err(controller::Error::Storage(journal::Error::Unsafe))
    ));
    let setup = journal::Journal::open(&opened.directory.join("profile-setup")).unwrap();
    assert!(setup.ids().unwrap().is_empty());
    assert_eq!(
        fs::metadata(lifecycle).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
