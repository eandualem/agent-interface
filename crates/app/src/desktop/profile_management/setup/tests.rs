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
        agent_name: "worker".into(),
        runtime: Some("codex".into()),
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
    // Nothing further can happen on this request, so it offers no provider action.
    assert!(!Command::Cancel.allowed(Some(&s)));
    assert_eq!(status(&s), "Agent not added");
    let retryable = |code: &str| {
        let mut s = snapshot();
        let mut result = event(&s);
        result.code = Some(code.into());
        result.retry = Some("same_request".into());
        s.first_result = Some(result);
        s
    };
    let timeout = retryable("launch_timeout");
    assert_eq!(status(&timeout), "Needs attention");
    assert!(Command::Retry.allowed(Some(&timeout)));
    assert!(
        outcome(&timeout)
            .unwrap()
            .contains("has not reported ready")
    );
    assert!(agent_waiting(&timeout));
    assert!(agent_waiting(&retryable("runtime_permission")));
    assert!(!agent_waiting(&retryable("runtime_missing")));
    assert!(!agent_waiting(&s));
    assert!(outcome(&retryable("agent_ready")).is_none());
    assert!(
        outcome(&retryable("something_new"))
            .unwrap()
            .contains("something_new")
    );
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
    let Page::History { ids, offset, rows } = history(&controller, None, None, 0, &live).unwrap()
    else {
        panic!("history expected")
    };
    assert_eq!(offset, 0);
    assert_eq!(ids.len(), PAGE_SIZE + 1);
    assert_eq!(rows.len(), PAGE_SIZE);
    assert!(
        rows.iter()
            .all(|(_, result)| *result == Err(Error::Invalid))
    );
    let Page::History { rows, .. } =
        history(&controller, None, Some(ids), PAGE_SIZE, &live).unwrap()
    else {
        panic!("history expected")
    };
    assert_eq!(rows.len(), 1);
    // A record that can't be read stays listed for every profile, so it can be forgotten.
    let Page::History { ids, .. } = history(&controller, Some("other"), None, 0, &live).unwrap()
    else {
        panic!("history expected")
    };
    assert_eq!(ids.len(), PAGE_SIZE + 1);
    live.store(false, Ordering::Release);
    assert!(matches!(
        history(&controller, None, None, 0, &live),
        Err(Error::Client(crate::profiles::Error::Cancelled))
    ));
}

#[test]
fn a_served_start_opens_only_a_running_agent_and_asks_again_only_without_an_answer() {
    use agent_interface_roster::profiles::{Error as Profile, setup::start::Readiness};
    let started = |ok, evidence: &[&str]| {
        Ok(Started {
            ok,
            name: "guide-2".into(),
            ready: if ok {
                Readiness::NotWaited
            } else {
                Readiness::Exited
            },
            evidence: evidence.iter().map(|line| line.to_string()).collect(),
        })
    };
    // The pane opens by the name the service answered with.
    assert_eq!(
        answer(started(true, &[])),
        Answer::Started("guide-2".into())
    );
    assert_eq!(
        answer(started(false, &["claude: not signed in\u{1b}[0m"])),
        Answer::Stopped("It stopped right after starting: claude: not signed in[0m".into())
    );
    assert!(matches!(answer(started(false, &[])), Answer::Stopped(_)));
    assert_eq!(
        answer(Err(AddError::Refused(
            "Directory does not exist: /x".into()
        ))),
        Answer::Stopped("Directory does not exist: /x".into())
    );
    assert!(matches!(
        answer(Err(AddError::Connection(Profile::Refused(500)))),
        Answer::Stopped(_)
    ));
    for unknown in [Profile::Unavailable, Profile::Invalid] {
        assert_eq!(
            answer(Err(AddError::Connection(unknown))),
            Answer::Unknown(None)
        );
    }
}

/// A Rust-service profile socket that answers each connection in turn and
/// returns the requests it read, lower-cased.
pub(in crate::desktop::profile_management) fn served_socket(
    replies: Vec<(u16, serde_json::Value)>,
) -> (
    tempfile::TempDir,
    Profile,
    std::thread::JoinHandle<Vec<String>>,
) {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let entry = serde_json::json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": path, "tmux_socket": "/run/profile/tmux.sock",
        "credential_reference": {"type": "socket"}
    });
    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in replies {
            // A client that stopped early fails the test instead of hanging it.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let mut request = String::from_utf8(header).unwrap().to_ascii_lowercase();
            if let Some(length) = request
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
            {
                let mut body = vec![0; length.trim().parse().unwrap()];
                stream.read_exact(&mut body).unwrap();
                request.push_str(&String::from_utf8(body).unwrap());
            }
            requests.push(request);
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
        requests
    });
    (directory, profile, server)
}

/// The profile handshake's answer, for the socket above.
pub(in crate::desktop::profile_management) fn served_identity() -> serde_json::Value {
    serde_json::json!({
        "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true
    })
}

#[test]
fn an_unknown_served_outcome_is_asked_again_under_its_own_request_id() {
    let identity = served_identity();
    // As the service's socket answers a start it sent on but heard nothing of.
    let unknown = serde_json::json!({"detail": {"code": "outcome_unknown",
        "message": "request 019a7c105c407a108b203c40d5e6f708 was sent to the profile's home, but what came of it is not known; it is not sent again"}});
    let started = serde_json::json!({"ok": true, "session": "guide", "name": "guide",
        "working_directory": "/a", "runtime": "claude", "model": null, "repo": "",
        "already_existed": false, "ready": "not_waited", "evidence": []});
    let taken = serde_json::json!({"detail": {"code": "conflict", "message": "guide is taken"}});
    let (_directory, profile, server) = served_socket(vec![
        (200, identity),
        (504, unknown),
        (200, started),
        (409, taken),
    ]);
    let first = "019a7c105c407a108b203c40d5e6f708";
    let mut request = Request {
        connection: Some(profile.connect().unwrap()),
        profile,
        start: Start::new("guide", std::path::Path::new("/a"), "claude", None, None).unwrap(),
        create: None,
        request_id: first.into(),
    };
    // No result yet: Try again asks about the same request, which then answers.
    assert_eq!(attempt(&request, false), Answer::Unknown(None));
    assert_eq!(attempt(&request, true), Answer::Started("guide".into()));
    // A refusal is final: what follows it is a new request with a new ID.
    request.request_id = "019a7c105c407a108b203c40d5e6f709".into();
    assert_eq!(
        attempt(&request, false),
        Answer::Stopped("guide is taken".into())
    );
    let requests = server.join().unwrap();
    let id = |at: usize| {
        requests[at]
            .lines()
            .find_map(|line| line.strip_prefix("x-backbone-request-id: "))
            .map(str::to_owned)
    };
    assert_eq!(id(1).as_deref(), Some(first));
    assert_eq!(id(2).as_deref(), Some(first));
    assert_eq!(id(3).as_deref(), Some("019a7c105c407a108b203c40d5e6f709"));
}

#[test]
fn an_unknown_start_is_not_headed_as_a_failure() {
    assert_eq!(
        heading("guide", Some(&Answer::Unknown(None))),
        "Not known whether guide started"
    );
    assert_eq!(
        heading("guide", Some(&Answer::Stopped("guide is taken".into()))),
        "guide didn't start"
    );
    // The earlier provider's starts keep their heading.
    assert_eq!(heading("guide", None), "guide didn't start");
}

#[test]
fn an_unreachable_profile_keeps_an_unresolved_start_and_its_request_id() {
    let unknown =
        serde_json::json!({"detail": {"code": "outcome_unknown", "message": "not known"}});
    let started = serde_json::json!({"ok": true, "session": "guide", "name": "guide",
        "working_directory": "/a", "runtime": "claude", "model": null, "repo": "",
        "already_existed": false, "ready": "not_waited", "evidence": []});
    let down = serde_json::json!({"detail": {"code": "unavailable", "message": "starting"}});
    // Add agent connects on every attempt: no connection is kept.
    let (_directory, profile, server) = served_socket(vec![
        (200, served_identity()),
        (504, unknown),
        (503, down.clone()),
        (200, served_identity()),
        (200, started),
        (503, down),
    ]);
    let first = "019a7c105c407a108b203c40d5e6f708";
    let request = Request {
        connection: None,
        profile,
        start: Start::new("guide", std::path::Path::new("/a"), "claude", None, None).unwrap(),
        create: None,
        request_id: first.into(),
    };
    assert_eq!(attempt(&request, false), Answer::Unknown(None));
    // Unreachable on Try again: nothing was sent, and the first send is
    // still the one to ask about.
    let unreached = attempt(&request, true);
    assert_eq!(
        unreached,
        Answer::Unknown(Some(
            "The profile could not be reached; start its service.".into()
        ))
    );
    assert_eq!(
        heading("guide", Some(&unreached)),
        "Not known whether guide started"
    );
    assert_eq!(attempt(&request, true), Answer::Started("guide".into()));
    // A first attempt that can't reach the profile sent nothing: it ends.
    let mut fresh = request.clone();
    fresh.request_id = "019a7c105c407a108b203c40d5e6f709".into();
    assert!(matches!(attempt(&fresh, false), Answer::Stopped(_)));
    let requests = server.join().unwrap();
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("post /api/agents/start "))
        .map(|request| {
            request
                .lines()
                .find_map(|line| line.strip_prefix("x-backbone-request-id: "))
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(posts, [first, first]);
}

#[test]
fn an_unresolved_start_is_the_same_request_while_what_it_sends_is_the_same() {
    use std::path::{Path, PathBuf};
    let (_directory, profile, _server) = served_socket(Vec::new());
    let start = |name: &str, dir: &str, model: Option<&str>, watch: Option<&str>| {
        Start::new(name, Path::new(dir), "claude", model, watch).unwrap()
    };
    let held = Request {
        connection: None,
        profile,
        start: start("guide", "/a", Some("opus"), Some("acme/app")),
        create: Some(PathBuf::from("/a")),
        request_id: "019a7c105c407a108b203c40d5e6f708".into(),
    };
    let made = PathBuf::from("/a");
    // The tool and model chosen again, or an edit typed back: the same start.
    assert!(same(
        &held,
        &start("guide", "/a", Some("opus"), Some("acme/app")),
        Some(&made)
    ));
    // A real difference is a new request.
    for other in [
        start("guide", "/a", None, Some("acme/app")),
        start("guide", "/a", Some("sonnet"), Some("acme/app")),
        start("guide-2", "/a", Some("opus"), Some("acme/app")),
        start("guide", "/b", Some("opus"), Some("acme/app")),
        start("guide", "/a", Some("opus"), None),
    ] {
        assert!(!same(&held, &other, Some(&made)));
    }
    assert!(!same(&held, &held.start, None));
}
