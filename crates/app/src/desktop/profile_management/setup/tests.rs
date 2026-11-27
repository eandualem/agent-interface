use super::*;

fn snapshot() -> Snapshot {
    Snapshot {
        key: "key-a".into(),
        revision: 7,
        phase: Phase::Submitted,
        profile_id: "0a1b2c3d4e5f".into(),
        agent_name: "worker".into(),
        runtime: Some("codex".into()),
        request_id: "request-a".into(),
        first_result: None,
        cancellation_result: None,
    }
}
fn event(s: &Snapshot) -> Event {
    Event {
        profile_id: s.profile_id.clone(),
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
        message: None,
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
    p.event.profile_id = "0a1b2c3d4e60".into();
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
    // A code that doesn't say the agent wasn't added: not known.
    assert_eq!(status(&s), "Not known whether it was added");
}

/// A saved request whose latest result, of the first agent or of its
/// cancellation, is `code` with `retry`.
fn answered(cancel: bool, status: &str, code: &str, retry: &str) -> Snapshot {
    let mut s = snapshot();
    let mut result = event(&s);
    result.step = None;
    result.status = status.into();
    result.code = Some(code.into());
    result.retry = Some(retry.into());
    if cancel {
        result.action = Action::CancelFirstAgent;
        s.phase = Phase::CancelPending;
        s.cancellation_result = Some(result);
    } else {
        s.first_result = Some(result);
    }
    s
}

#[test]
fn a_backbone_too_old_for_the_orchestrator_says_so_and_other_refusals_are_unchanged() {
    // What a Backbone from before the choice says of the field it doesn't know.
    let too_old = "the request is not one this version takes: unknown field `orchestrator`, \
                   expected one of `name`, `directory`";
    for (message, old) in [
        (too_old, true),
        (
            "brief.template `orchestrator-v1` is not one setup has",
            false,
        ),
        ("the agent's name is not one setup takes", false),
    ] {
        let mut s = answered(false, "failed", "invalid_request", "new_request");
        s.first_result.as_mut().unwrap().message = Some(message.into());
        let (text, _) = outcome(&s).unwrap();
        assert_eq!(
            text.starts_with("This Agent Backbone is too old"),
            old,
            "{message}"
        );
        assert_eq!(
            text.starts_with("Setup refused the request"),
            !old,
            "{message}"
        );
        // Nothing was written, so a new request follows with the choices changed.
        assert!(text.ends_with("change what you need and start again."));
    }
}
#[test]
fn every_answer_s_words_agree_with_its_status_and_buttons() {
    // Every code setup gives a first agent or its cancellation, and one it
    // doesn't know. Those the app has words of its own for show them;
    // every other shows setup's message.
    let own = [
        "cancellation_unsafe",
        "agent_conflict",
        "runtime_missing",
        "invalid_request",
        "service_unavailable",
    ];
    let codes = [
        "agent_ready",
        "request_cancelled",
        "starter_conflict",
        "launch_failed",
        "installation_conflict",
        "request_conflict",
        "request_expired",
        "outcome_unknown",
        "setup_failed",
        "operator_required",
        "new_code",
    ];
    for cancel in [false, true] {
        for given in ["completed", "failed", "action_required", "paused"] {
            for code in own.into_iter().chain(codes) {
                for retry in ["same_request", "new_request", "none", "later"] {
                    let row = format!("cancel {cancel}: {given} {code} {retry}");
                    let mut s = answered(cancel, given, code, retry);
                    // What the decoder confirms of these, and nothing else.
                    let result = if cancel {
                        s.cancellation_result.as_mut()
                    } else {
                        s.first_result.as_mut()
                    }
                    .unwrap();
                    result.message = Some("the profile's words".into());
                    if given == "completed" && code == "request_cancelled" && cancel {
                        result.cancelled = true;
                    }
                    if given == "completed" && code == "agent_ready" && !cancel {
                        result.ready_agent = Some("worker".into());
                    }
                    let line = status(&s);
                    let Some((text, shown)) = outcome(&s) else {
                        assert!(
                            matches!(line, "Agent started" | "Safely cancelled"),
                            "{row}"
                        );
                        assert!(s.closed(), "{row}");
                        continue;
                    };
                    assert_eq!(shown, code, "{row}");
                    // Setup's words only where the app has none of its own.
                    assert_eq!(
                        text.starts_with("the profile's words"),
                        !own.contains(&code),
                        "{row}: {text}"
                    );
                    let (again, again_cancel) = (
                        Command::Retry.allowed(Some(&s)),
                        Command::RetryCancel.allowed(Some(&s)),
                    );
                    // Only a final retry closes it; a cancellation's leaves the
                    // original as it was, unless it is too old to run now.
                    let last = matches!(retry, "new_request" | "none");
                    assert_eq!(
                        s.closed(),
                        last && (!cancel || code == "request_expired"),
                        "{row}"
                    );
                    // The same request or cancellation only where its retry
                    // says so; the original from a cancellation that didn't
                    // cancel it.
                    assert_eq!(
                        text.contains("same request"),
                        !cancel && again,
                        "{row}: {text}"
                    );
                    assert_eq!(
                        !cancel && again,
                        !cancel && retry == "same_request",
                        "{row}"
                    );
                    assert_eq!(
                        text.contains("same cancellation"),
                        again_cancel,
                        "{row}: {text}"
                    );
                    assert_eq!(again_cancel, cancel && retry == "same_request", "{row}");
                    assert_eq!(
                        text.contains("send it again"),
                        cancel && again && !again_cancel,
                        "{row}: {text}"
                    );
                    // A cancellation that didn't confirm says only that, and
                    // to look at the agent first.
                    assert!(!text.contains("isn't cancelled"), "{row}: {text}");
                    assert_eq!(
                        text.contains("isn't confirmed"),
                        cancel && again && !again_cancel && code != "cancellation_unsafe",
                        "{row}: {text}"
                    );
                    assert_eq!(
                        text.contains("Look at the agent first"),
                        code == "cancellation_unsafe" || (cancel && again && !again_cancel),
                        "{row}: {text}"
                    );
                    // A new request only where one may follow, and look first
                    // wherever what came of it isn't known.
                    assert_eq!(
                        text.contains("start again"),
                        s.closed() && retry == "new_request",
                        "{row}: {text}"
                    );
                    assert_eq!(
                        text.contains("look at the workspace")
                            || text.contains("Look at the workspace"),
                        !again && !again_cancel && !(s.closed() && retry == "new_request"),
                        "{row}: {text}"
                    );
                    // No word that the agent wasn't added, or was.
                    for claim in [
                        "not added",
                        "wasn't added",
                        "nothing was sent",
                        "never ran",
                        "nothing changed",
                        "was added",
                    ] {
                        assert!(!text.contains(claim), "{row}: {text}");
                    }
                    // Open or closed, as the status line and buttons say.
                    if s.closed() {
                        assert_eq!(line, "Not known whether it was added", "{row}");
                        assert!(
                            !again && !again_cancel && !Command::Cancel.allowed(Some(&s)),
                            "{row}"
                        );
                    } else {
                        assert_eq!(line, "Needs attention", "{row}");
                        assert_eq!(Command::Cancel.allowed(Some(&s)), !cancel, "{row}");
                    }
                }
            }
        }
    }
    // A cancellation's final answer that didn't cancel leaves the original
    // open, unless it is too old to run now, which closes it.
    for (code, retry, open) in [
        ("cancellation_unsafe", "none", true),
        ("request_conflict", "new_request", true),
        ("invalid_request", "new_request", true),
        ("request_expired", "none", false),
    ] {
        let s = answered(true, "failed", code, retry);
        assert_eq!(Command::Retry.allowed(Some(&s)), open, "{code}");
        assert_eq!(s.closed(), !open, "{code}");
    }
    // The guide's main button, as rendered: the same request while it may
    // be sent again, a new one only once it is closed, and no button while
    // it is open with nothing it may send.
    for (retry, button) in [
        ("same_request", Some(("Try again", Then::Replay))),
        ("none", Some(("Start a new request", Then::Fresh))),
        ("new_request", Some(("Start a new request", Then::Fresh))),
        ("later", None),
    ] {
        let s = answered(false, "action_required", "outcome_unknown", retry);
        assert_eq!(main_button(None, Some(&s)), button, "{retry}");
    }
    let mut saved = snapshot();
    saved.phase = Phase::Prepared;
    assert_eq!(
        main_button(None, Some(&saved)),
        Some(("Try again", Then::Replay))
    );
    assert_eq!(
        main_button(None, None),
        Some(("Start a new request", Then::Fresh))
    );
    // Add agent's, as it always was: Try again, the same start while what
    // came of it isn't known, else a new one.
    for (answer, then) in [
        (Some(Answer::Unknown(None)), Then::Replay),
        (Some(Answer::Unknown(Some("why".into()))), Then::Replay),
        (Some(Answer::Stopped("guide is taken".into())), Then::Fresh),
        (None, Then::Fresh),
    ] {
        assert_eq!(main_button(Some(&answer), None), Some(("Try again", then)));
    }
    // Setup said nothing: the app says so, never an empty line.
    let s = answered(false, "failed", "new_code", "none");
    assert!(
        outcome(&s)
            .unwrap()
            .0
            .starts_with("Setup said nothing more.")
    );
    // Started: its terminal is the next step.
    let mut s = answered(false, "completed", "agent_ready", "none");
    s.first_result.as_mut().unwrap().ready_agent = Some("worker".into());
    assert!(started(&s) && outcome(&s).is_none() && s.closed());
    assert_eq!(status(&s), "Agent started");
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
    let started = |ok, reason: Option<&str>| {
        Ok(Started {
            ok,
            name: "guide-2".into(),
            ready: if ok {
                Readiness::NotWaited
            } else {
                Readiness::Exited
            },
            reason: reason.map(str::to_owned),
            evidence: vec!["First observation".into(), "Last observation".into()],
        })
    };
    // The pane opens by the name the service answered with.
    assert_eq!(
        answer(started(true, None)),
        Answer::Started("guide-2".into())
    );
    assert_eq!(
        answer(started(false, Some("No space\nleft."))),
        Answer::Stopped("No space left.".into())
    );
    assert_eq!(answer(started(false, None)), Answer::Stopped("The CLI exited right after starting, so the agent is not running. Run the CLI in a terminal in the agent's folder to see why, then start it again.".into()));
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
            // The version check each connection makes first is answered
            // without taking a reply or being recorded.
            let (mut stream, request) = loop {
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
                if !request.starts_with("get /health ") {
                    break (stream, request);
                }
                let version = agent_interface_roster::profiles::MINIMUM_BACKBONE.to_string();
                let body = serde_json::json!({ "version": version }).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            };
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
        elsewhere: None,
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
    // A start read from the journal keeps its heading.
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
        elsewhere: None,
    };
    assert_eq!(attempt(&request, false), Answer::Unknown(None));
    // Unreachable on Try again: nothing was sent, and the first send is
    // still the one to ask about.
    let unreached = attempt(&request, true);
    assert_eq!(
        unreached,
        Answer::Unknown(Some("The profile could not be reached.".into()))
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
fn a_fresh_start_sends_only_its_open_request_until_it_is_closed() {
    assert_eq!(saved_open(None), None);
    // Saved and never sent: it is sent.
    let mut saved = snapshot();
    saved.phase = Phase::Prepared;
    assert_eq!(saved_open(Some(&saved)), Some(true));
    for (retry, again) in [
        ("same_request", Some(true)),
        // Open, but not sent again: no Try again, and no new request.
        ("later", Some(false)),
        ("new_request", None),
        ("none", None),
    ] {
        let s = answered(false, "action_required", "outcome_unknown", retry);
        assert_eq!(saved_open(Some(&s)), again, "{retry}");
    }
}

#[test]
fn every_unreadable_setup_record_offers_forget() {
    use crate::profiles::journal;
    for error in [
        Error::Invalid,
        Error::Storage(journal::Error::Invalid),
        Error::Storage(journal::Error::TooLarge),
    ] {
        assert!(unreadable(error), "{error:?}");
    }
    assert!(!unreadable(Error::Missing));
    assert!(!unreadable(Error::Storage(journal::Error::Unavailable)));
}

#[test]
fn a_start_on_another_machine_says_where_it_is() {
    assert_eq!(
        started_elsewhere("app", "linux-box"),
        (
            "app started".to_owned(),
            "Started on linux-box; it appears in the panel.".to_owned()
        )
    );
}
