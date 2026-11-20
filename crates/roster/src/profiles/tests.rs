use super::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    thread,
    time::Instant,
};

const REG: &str = "1234567890abcdef1234567890abcdef";

fn inspected(registration: Value) -> Value {
    json!({"name":"worker","known":true,"runtime":"codex","registration_id":registration,"recent_deliveries":[{"kind":"direct_message","outcome":"queued","preview":"fixture","created_at":"2026-11-02T07:00:00Z"}]})
}

#[test]
fn retained_inspection_cannot_replace_or_drop_registration() {
    use super::runtime_context as context;
    let registration = REG;
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let mut missing = inspected(json!(registration));
    missing.as_object_mut().unwrap().remove("registration_id");
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, inspected(json!(registration))),
            (200, inspected(json!("abcdef1234567890abcdef1234567890"))),
            (200, inspected(Value::Null)),
            (200, missing),
            (200, inspected(Value::Null)),
        ],
    );
    let connection = profile.connect().unwrap();
    for invalid in ["", "UPPERCASE1234567890ABCDEF12345678", "../other", "a"] {
        assert_eq!(
            context::inspect(connection.clone(), "worker", Some(invalid))
                .err()
                .unwrap(),
            context::Error::Invalid
        );
    }
    assert_eq!(
        context::inspect(connection.clone(), "../other", None)
            .err()
            .unwrap(),
        context::Error::Invalid
    );
    let card = context::inspect(connection.clone(), "worker", None).unwrap();
    assert_eq!(
        card.target.refresh().err(),
        Some(context::Error::RegistrationChanged)
    );
    assert_eq!(
        card.target.refresh().err(),
        Some(context::Error::RegistrationChanged)
    );
    assert_eq!(card.target.refresh().err(), Some(context::Error::Invalid));
    assert_eq!(
        context::inspect(connection, "worker", None).err().unwrap(),
        context::Error::Unavailable
    );
    assert_eq!(server.join().unwrap().len(), 6);
}

#[test]
fn same_named_agent_contexts_remain_with_their_profile() {
    use super::runtime_context as context;
    let mut profiles = Vec::new();
    for label in ["a", "b"] {
        let (directory, listener, socket) = crate::transport::tests::local_listener();
        let (mut profile, mut identity) = profile_at(directory.path(), &socket);
        profile.item.profile_id = format!("profile-{label}");
        identity["profile_id"] = json!(profile.item.profile_id);
        let mut agent = inspected(json!(REG));
        agent["recent_deliveries"][0]["preview"] = json!(format!("from-{label}"));
        let server = serve(
            listener,
            vec![(200, identity), (200, agent.clone()), (200, agent)],
        );
        let card = context::inspect(profile.connect().unwrap(), "worker", None).unwrap();
        profiles.push((directory, server, card));
    }
    let (_b_dir, b_server, b) = profiles.pop().unwrap();
    let (_a_dir, a_server, a) = profiles.pop().unwrap();
    assert_eq!(b.target.refresh().unwrap().received[0].preview, "from-b");
    assert_eq!(a.target.refresh().unwrap().received[0].preview, "from-a");
    for (server, label) in [(a_server, "a"), (b_server, "b")] {
        for request in server.join().unwrap() {
            assert!(request.contains(&format!("x-backbone-profile-id: profile-{label}\r\n")));
        }
    }
}

fn profile_at(directory: &Path, socket: &str) -> (Profile, Value) {
    let mut entry = served_entry(socket);
    entry["tmux_socket"] = json!(directory.join("tmux.sock"));
    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    (profile, handshake(entry))
}

/// The service's `/api/profile` reply for a listed `entry`.
fn handshake(mut entry: Value) -> Value {
    for field in ["state", "socket", "tmux_socket", "credential_reference"] {
        entry.as_object_mut().unwrap().remove(field);
    }
    entry
}

fn serve(listener: UnixListener, replies: Vec<(u16, Value)>) -> thread::JoinHandle<Vec<String>> {
    serve_with_hook(listener, replies, |_| {})
}

fn serve_with_hook(
    listener: UnixListener,
    replies: Vec<(u16, Value)>,
    mut hook: impl FnMut(usize) + Send + 'static,
) -> thread::JoinHandle<Vec<String>> {
    thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut requests = Vec::new();
        for (at, (status, body)) in replies.into_iter().enumerate() {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() < 8192);
            }
            let mut request = String::from_utf8(header).unwrap().to_ascii_lowercase();
            if let Some(length) = request
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
            {
                let mut body = vec![0; length.trim().parse().unwrap()];
                stream.read_exact(&mut body).unwrap();
                request.push_str(&String::from_utf8(body).unwrap().to_ascii_lowercase());
            }
            requests.push(request);
            hook(at);
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    })
}

#[test]
fn connection_pins_identity_through_followup_and_replacement() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let mismatch = json!({"detail": {"code": "binding_mismatch", "message": "Profile binding does not match."}});
    let server = serve(
        listener,
        vec![(200, identity), (200, json!({})), (409, mismatch)],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(connection.identity().lifecycle_id, "5f2c9e0d1b7a4e38");
    assert_eq!(connection.tmux_socket(), directory.path().join("tmux.sock"));
    let response = connection
        .response("/api/example", Some(vec![]), 1024)
        .unwrap();
    assert_eq!(response.status, 200);
    // The same path now refuses the connection. No unbound retry follows.
    assert_eq!(
        connection
            .response("/api/example", Some(vec![]), 1024)
            .err(),
        Some(Error::BindingMismatch)
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].starts_with("post /api/example "));
    for request in requests {
        assert!(request.contains("x-backbone-profile-id: 0a1b2c3d4e5f\r\n"));
        assert!(!request.contains("authorization"));
    }
}

#[test]
fn bound_shell_request_carries_no_environment_and_checks_its_reply() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let reply = |shell_id: &str, profile: &Value| {
        json!({"schema_version": 1, "profile_id": profile,
            "instance_id": identity["instance_id"],
            "shell_id": shell_id, "session": "opaque-session", "created": true})
    };
    let own = &identity["profile_id"];
    let server = serve(
        listener,
        vec![
            (200, identity.clone()),
            (200, reply("123456abcdef", own)),
            (200, reply("123456abcdef", &json!("1a2b3c4d5e6f"))),
            (200, reply("foreign000000", own)),
            (503, json!({"detail": {"code": "shell_unavailable"}})),
            (404, json!({})),
        ],
    );
    let connection = profile.connect().unwrap();
    let ensure = |cwd| connection.ensure_shell("123456abcdef", cwd);
    assert_eq!(ensure(Some("/work")).as_deref(), Ok("opaque-session"));
    assert_eq!(ensure(None).err(), Some(Error::BindingMismatch));
    assert_eq!(ensure(None).err(), Some(Error::Invalid));
    assert_eq!(ensure(None).err(), Some(Error::Refused(503)));
    assert_eq!(ensure(None).err(), Some(Error::Unsupported));
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("post /api/profile/shells "));
    let body = requests[1].rsplit("\r\n\r\n").next().unwrap();
    let keys: Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        keys,
        json!({"schema_version": 1, "shell_id": "123456abcdef", "cwd": "/work"})
    );
}

#[test]
fn starting_a_stopped_agent_uses_the_bound_route_and_reports_the_outcome() {
    use crate::lifecycle::Outcome;
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let started = |ok: bool, existed: bool, ready: &str| json!({"ok": ok, "session": "worker", "already_existed": existed, "ready": ready});
    let probe = listener.try_clone().unwrap();
    let server = serve(
        listener,
        vec![
            (200, identity.clone()),
            (200, started(true, false, "not_waited")),
            (200, started(true, true, "ready")),
            (200, started(false, false, "exited")),
            (400, json!({"detail": "no saved conversation"})),
        ],
    );
    let connection = profile.connect().unwrap();
    assert!(matches!(
        connection.start_agent("worker", false),
        Outcome::Accepted(_)
    ));
    assert_eq!(
        connection.start_agent("worker", true),
        Outcome::AlreadyRunning
    );
    assert!(matches!(
        connection.start_agent("worker", false),
        Outcome::Refused(_)
    ));
    assert!(matches!(
        connection.start_agent("worker", true),
        Outcome::Refused(_)
    ));
    assert!(matches!(
        connection.start_agent("../other", false),
        Outcome::Refused(_)
    ));
    let requests = server.join().unwrap();
    // The handshake, then one request for each start; the invalid name sends
    // none, and nothing connects again.
    assert_eq!(requests.len(), 5);
    assert_eq!(
        probe.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(requests[1].starts_with("post /api/agents/worker/start "));
    let body = |request: &str| -> Value {
        serde_json::from_str(request.rsplit("\r\n\r\n").next().unwrap()).unwrap()
    };
    assert_eq!(body(&requests[1]), json!({"resume": false, "wait": false}));
    assert_eq!(body(&requests[2]), json!({"resume": true, "wait": false}));
}

#[test]
fn a_reply_of_exactly_the_limit_is_read_and_one_byte_more_is_refused() {
    use crate::transport::tests::{ok, serve_on, served_connection};
    let (_directory, connection, listener) = served_connection();
    let chunk = |data: &str| format!("{:x}\r\n{data}\r\n", data.len());
    let half = "x".repeat(RESPONSE_LIMIT / 2);
    let replies = [
        ok(&"x".repeat(RESPONSE_LIMIT)),
        ok(&"x".repeat(RESPONSE_LIMIT + 1)),
        // The same excess in several chunks, with no length declared up front.
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{}{}0\r\n\r\n",
            chunk(&half),
            chunk(&format!("{half}x")),
        ),
    ];
    let mut read = Vec::new();
    for reply in replies {
        let server = serve_on(listener.try_clone().unwrap(), None, reply, Duration::ZERO);
        read.push(
            connection
                .response("/api/example", None, RESPONSE_LIMIT)
                .map(|response| response.body.len()),
        );
        server.join().unwrap();
    }
    assert_eq!(
        read,
        [Ok(RESPONSE_LIMIT), Err(Error::Invalid), Err(Error::Invalid)]
    );
}

#[test]
fn an_unreadable_or_lost_start_reply_is_unknown_after_one_request() {
    use crate::lifecycle::Outcome;
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let probe = listener.try_clone().unwrap();
    let server = serve(
        listener.try_clone().unwrap(),
        vec![(200, identity), (200, json!("not json"))],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(connection.start_agent("worker", false), Outcome::Unknown);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("post /api/agents/worker/start "));
    // The connection closes before any answer.
    let lost = crate::transport::tests::serve_on(listener, None, String::new(), Duration::ZERO);
    assert_eq!(connection.start_agent("worker", false), Outcome::Unknown);
    assert!(
        lost.join()
            .unwrap()
            .starts_with("POST /api/agents/worker/start ")
    );
    // Neither start was sent again.
    assert_eq!(
        probe.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn removing_an_agent_stops_it_first_and_treats_a_missing_one_as_removed() {
    const REG: &str = "1234567890abcdef1234567890abcdef";
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let stopped = json!({"ok": true, "session": "worker"});
    let changed = json!({"detail": {"code": "registration_changed"}});
    let server = serve(
        listener,
        vec![
            (200, identity.clone()),
            (200, stopped.clone()),
            (200, json!({"ok": true, "name": "worker"})),
            (404, json!({})),
            (200, stopped.clone()),
            (404, json!({})),
            (200, stopped.clone()),
            (409, json!({"detail": "'worker' is running"})),
            (409, changed.clone()),
            (200, stopped),
            (409, changed),
        ],
    );
    let connection = profile.connect().unwrap();
    // Removed; a 404 on stop ends there; a 404 on delete counts as removed.
    assert_eq!(connection.remove_agent("worker", REG), Ok(()));
    assert_eq!(connection.remove_agent("worker", REG), Ok(()));
    assert_eq!(connection.remove_agent("worker", REG), Ok(()));
    // The delete is refused while the agent runs.
    assert_eq!(
        connection.remove_agent("worker", REG).err(),
        Some(Error::Refused(409))
    );
    // The registration changed before the stop, then between stop and delete.
    assert_eq!(
        connection.remove_agent("worker", REG).err(),
        Some(Error::RegistrationChanged)
    );
    assert_eq!(
        connection.remove_agent("worker", REG).err(),
        Some(Error::RegistrationChanged)
    );
    assert_eq!(
        connection.remove_agent("../other", REG).err(),
        Some(Error::Invalid)
    );
    assert_eq!(
        connection.remove_agent("worker", "invalid").err(),
        Some(Error::Invalid)
    );
    let requests = server.join().unwrap();
    let query = format!("?expected_registration_id={REG} ");
    assert!(requests[1].starts_with(&format!("post /api/agents/worker/stop{query}")));
    assert!(requests[2].starts_with(&format!("delete /api/agents/worker{query}")));
    assert!(requests[3].starts_with("post /api/agents/worker/stop"));
    assert!(requests[5].starts_with("delete /api/agents/worker"));
    assert!(requests[7].starts_with("delete /api/agents/worker"));
    assert_eq!(requests.len(), 11);
}

#[test]
fn agent_list_reads_again_while_registrations_settle_but_not_forever() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let settling = (
        409,
        json!({"detail": {"code": "registration_changed", "message": "Changed."}}),
    );
    let mut replies = vec![
        (200, identity.clone()),
        settling.clone(),
        (200, json!({"items": [], "total": 0})),
        (200, json!({"items": []})),
    ];
    replies.extend(std::iter::repeat_n(settling, 41));
    let server = serve(listener, replies);
    let connection = profile.connect().unwrap();
    assert!(connection.roster().is_ok());
    assert_eq!(connection.roster().err(), Some(Error::Refused(409)));
    assert_eq!(server.join().unwrap().len(), 45);
}

#[test]
fn bound_roster_preserves_typed_failures_and_decodes_success() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let required =
        json!({"detail": {"code": "binding_required", "message": "Profile binding is required."}});
    let required_again = required.clone();
    let server = serve(
        listener,
        vec![
            (200, identity),
            (409, required),
            (403, json!({})),
            (200, json!({"items": "invalid"})),
            (200, json!({"items": [{"name": "worker", "state": "busy"}]})),
            (
                200,
                json!({"items": [{"name": "fix", "status": "active", "initiator": "worker", "members": ["fix-coder"]}]}),
            ),
            (200, json!({"items": []})),
            // An unreadable swarm list leaves the roster itself usable.
            (500, json!({})),
            // A binding refused on the swarm read is still a refused binding.
            (200, json!({"items": []})),
            (409, required_again),
        ],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(connection.roster().err(), Some(Error::BindingRequired));
    assert_eq!(connection.roster().err(), Some(Error::Refused(403)));
    assert_eq!(connection.roster().err(), Some(Error::Invalid));
    let roster = connection.roster().unwrap();
    assert_eq!(roster.states["worker"], "busy");
    assert!(roster.error.is_none() && roster.observed_at.is_some());
    assert_eq!(roster.swarms[0].members, ["fix-coder"]);
    let empty = connection.roster().unwrap();
    assert!(empty.states.is_empty() && empty.swarms.is_empty());
    assert_eq!(connection.roster().err(), Some(Error::BindingRequired));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 10);
    for (index, request) in requests.iter().enumerate().skip(1) {
        let path = if [5, 7, 9].contains(&index) {
            "/api/swarms"
        } else {
            "/api/agents"
        };
        assert!(request.starts_with(&format!("get {path} ")));
        assert!(request.contains("x-backbone-profile-id: 0a1b2c3d4e5f\r\n"));
    }
}

#[test]
fn handshake_rejects_another_profile_machine_or_run_and_an_unsupported_service() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let wrong = |field: &str, value: Value| {
        let mut reply = identity.clone();
        reply[field] = value;
        (200, reply)
    };
    let required =
        json!({"detail": {"code": "binding_required", "message": "Profile binding is required."}});
    let server = serve(
        listener,
        vec![
            wrong("profile_id", json!("1a2b3c4d5e6f")),
            wrong("machine", json!({"id": "other-machine"})),
            wrong("instance_id", json!("0190c3a2c1107a3b8c9d0e1f2a3b4c5e")),
            (404, json!({})),
            (409, required),
            wrong("protocol_version", json!(2)),
        ],
    );
    assert_eq!(profile.connect().err(), Some(Error::BindingMismatch));
    assert_eq!(profile.connect().err(), Some(Error::BindingMismatch));
    assert_eq!(profile.connect().err(), Some(Error::BindingMismatch));
    assert_eq!(profile.connect().err(), Some(Error::Unsupported));
    assert_eq!(profile.connect().err(), Some(Error::BindingRequired));
    assert_eq!(profile.connect().err(), Some(Error::Unsupported));
    assert_eq!(server.join().unwrap().len(), 6);
}

mod library_snapshots;

#[test]
fn adding_a_document_keeps_the_services_reason() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let refused =
        json!({"detail": {"code": "invalid_request", "message": "HTML exceeds the limit"}});
    let server = serve(
        listener,
        vec![
            (200, identity.clone()),
            (201, json!({"record": {"id": 7}, "created": true})),
            (422, refused),
            (403, json!({})),
        ],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(
        connection.add_document("Overview", "<h1>Overview</h1>"),
        Ok(7)
    );
    assert_eq!(
        connection.add_document("Overview", "<h1>Overview</h1>"),
        Err(AddError::Refused("HTML exceeds the limit".into()))
    );
    assert_eq!(
        connection.add_document("Overview", "<h1>Overview</h1>"),
        Err(AddError::Connection(Error::Refused(403)))
    );
    let requests = server.join().unwrap();
    let body = |request: &str| -> Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    };
    assert!(requests[1].starts_with("post /api/documents "));
    let document = body(&requests[1]);
    // The fixture server lowercases what it records.
    assert_eq!(
        (&document["author"], &document["title"], &document["html"]),
        (
            &json!("operator"),
            &json!("overview"),
            &json!("<h1>overview</h1>")
        )
    );
    assert_eq!(document["supersedes_id"], Value::Null);
    let retried = body(&requests[2]);
    assert_eq!(document["request_id"], retried["request_id"]);
    assert_eq!(document["request_id"].as_str().unwrap().len(), 68);
}

fn served_entry(socket: &str) -> Value {
    json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": socket, "tmux_socket": "/run/profile/tmux.sock",
        "credential_reference": {"type": "socket"}
    })
}

#[test]
fn rust_service_entries_list_and_connect_without_a_key() {
    let (_directory, listener, socket) = crate::transport::tests::local_listener();
    let entry = served_entry(&socket);
    let failed = json!({"state": "failed", "profile_id": "9f8e7d6c5b4a", "code": "database_unreachable", "reason": "fixture"});
    let later = json!({"state": "moving", "profile_id": "1a2b3c4d5e6f"});
    let listed = json!({"profiles": [entry, failed, later]});
    let items = list(&serde_json::to_vec(&listed).unwrap()).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].state, State::Ready);
    assert_eq!(
        (&items[1].state, items[1].display_name.as_str()),
        (&State::Failed, "9f8e7d6c5b4a")
    );
    assert_eq!(
        Profile::parse(&serde_json::to_vec(&failed).unwrap())
            .unwrap()
            .connect()
            .err(),
        Some(Error::NotReady)
    );
    assert_eq!(
        Profile::parse(&serde_json::to_vec(&later).unwrap()).err(),
        Some(Error::Unsupported)
    );
    // A reason that can't be shown as given is left out.
    let reason = |reason: &str| {
        let mut entry = failed.clone();
        entry["reason"] = json!(reason);
        Profile::parse(&serde_json::to_vec(&entry).unwrap())
            .unwrap()
            .item()
            .reason
            .clone()
    };
    assert_eq!(
        reason("Its folder is not readable.").as_deref(),
        Some("Its folder is not readable.")
    );
    for unshown in ["", "   ", "two\nlines", "bell\u{7}"] {
        assert_eq!(reason(unshown), None, "{unshown:?}");
    }
    // Only the object the service writes is read, never a positional array.
    assert_eq!(list(b"[[]]").err(), Some(Error::Invalid));
    assert_eq!(
        Profile::parse(br#"["failed", "9f8e7d6c5b4a", null, null]"#).err(),
        Some(Error::Invalid)
    );

    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    let identity = handshake(entry.clone());
    let mut restarted = identity.clone();
    restarted["instance_id"] = json!("0190c3a2c1107a3b8c9d0e1f2a3b4c5e");
    let agents = json!({"items": [{"name": "worker", "state": "working"}]});
    // The service's shell reply names its run, with no installation or generation.
    let opened = json!({
        "schema_version": 1, "profile_id": "0a1b2c3d4e5f",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "shell_id": "0123456789ab", "session": "shell-0123456789ab", "created": true
    });
    let mut foreign = opened.clone();
    foreign["profile_id"] = json!("1a2b3c4d5e6f");
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, agents),
            (200, json!({"items": []})),
            (200, opened.clone()),
            (200, foreign),
            (200, restarted.clone()),
            (200, restarted),
        ],
    );
    let connection = profile.connect().unwrap();
    // The machine stands in for the generation, so a restart keeps the identity.
    assert_eq!(connection.identity().lifecycle_id, "5f2c9e0d1b7a4e38");
    assert_eq!(
        connection.tmux_socket(),
        Path::new("/run/profile/tmux.sock")
    );
    assert_eq!(connection.roster().unwrap().states["worker"], "working");
    let shell = || connection.ensure_shell("0123456789ab", None);
    assert_eq!(shell().as_deref(), Ok("shell-0123456789ab"));
    assert_eq!(shell().err(), Some(Error::BindingMismatch));
    // A restarted service still serves this connection; a new one reads its entry again.
    assert_eq!(connection.verify(), Ok(()));
    assert_eq!(profile.connect().err(), Some(Error::BindingMismatch));
    for request in server.join().unwrap() {
        assert!(request.contains("x-backbone-profile-id: 0a1b2c3d4e5f\r\n"));
        assert!(!request.contains("authorization") && !request.contains("lifecycle"));
    }
}

#[test]
fn the_rust_service_takes_a_skill_folders_files() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let entry = served_entry(&socket);
    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    let mut identity = entry.clone();
    for field in ["state", "socket", "tmux_socket", "credential_reference"] {
        identity.as_object_mut().unwrap().remove(field);
    }
    let folder = directory.path().join("page-layout");
    fs::create_dir_all(folder.join("scripts")).unwrap();
    fs::create_dir_all(folder.join(".git")).unwrap();
    fs::write(folder.join("SKILL.md"), "---\nname: page-layout\n---\n").unwrap();
    fs::write(folder.join("scripts/check.sh"), "#!/bin/sh\n").unwrap();
    for (file, mode) in [("SKILL.md", 0o644), ("scripts/check.sh", 0o755)] {
        fs::set_permissions(folder.join(file), fs::Permissions::from_mode(mode)).unwrap();
    }
    fs::write(folder.join(".git/HEAD"), "ref").unwrap();
    std::os::unix::fs::symlink(folder.join("SKILL.md"), folder.join("link.md")).unwrap();
    std::os::unix::fs::symlink(folder.join("scripts"), folder.join("linked")).unwrap();
    let changed = json!({
        "name": "page-layout", "description": "Lay out a page.", "tags": ["all"],
        "error": null, "relinked": [], "warnings": ["builder was not reached"]
    });
    let refused =
        json!({"detail": {"code": "conflict", "message": "page-layout is already in the store"}});
    let server = serve(
        listener,
        vec![(200, identity), (200, changed), (409, refused)],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(
        connection.add_skill(&folder),
        Ok(AddedSkill {
            name: "page-layout".into(),
            warnings: vec!["builder was not reached".into()],
        })
    );
    assert_eq!(
        connection.add_skill(&folder),
        Err(AddError::Refused(
            "page-layout is already in the store".into()
        ))
    );
    // The fixture server keeps each request in lower case.
    let requests = server.join().unwrap();
    let body: Value = serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({
            "name": "page-layout", "tags": ["all"], "replace": false, "actor": "app",
            "files": [
                {"path": "skill.md", "mode": 0o644,
                 "content_base64": "ls0tcm5hbwu6ihbhz2utbgf5b3v0ci0tlqo="},
                {"path": "scripts/check.sh", "mode": 0o755,
                 "content_base64": "iyevymlul3nocg=="}
            ]
        })
    );
}

#[test]
fn a_skill_folder_is_read_within_the_services_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let mut deep = directory.path().join("deep");
    for _ in 0..16 {
        deep = deep.join("d");
    }
    fs::create_dir_all(&deep).unwrap();
    let wide = directory.path().join("wide");
    fs::create_dir(&wide).unwrap();
    for index in 0..=2_000 {
        fs::write(wide.join(format!("{index}.md")), "").unwrap();
    }
    for (folder, reason) in [("deep", "nested too deeply"), ("wide", "2,000 entries")] {
        let Err(AddError::Refused(refusal)) = served::skill(&directory.path().join(folder)) else {
            panic!("{folder} was read");
        };
        assert!(refusal.contains(reason), "{refusal}");
    }
}

#[test]
fn a_served_first_agent_starts_under_its_request_id_and_keeps_a_refusal() {
    use super::setup::start::{Readiness, Start};
    let (_directory, listener, socket) = crate::transport::tests::local_listener();
    let entry = served_entry(&socket);
    let mut identity = entry.clone();
    for field in ["state", "socket", "tmux_socket", "credential_reference"] {
        identity.as_object_mut().unwrap().remove(field);
    }
    // As the service's agent-started.json, and the same with nothing running.
    let started = json!({"ok": true, "session": "guide", "name": "guide",
        "working_directory": "/home/sam/Agents/guide", "runtime": "claude", "model": "opus",
        "repo": "", "already_existed": false, "ready": "not_waited", "evidence": []});
    let mut exited = started.clone();
    exited["ok"] = json!(false);
    exited["ready"] = json!("exited");
    exited["evidence"] = json!(["claude exited"]);
    let refusal =
        json!({"detail": {"code": "invalid_request", "message": "Directory does not exist: /x"}});
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, started),
            (200, exited),
            (400, refusal),
        ],
    );
    let connection = Profile::parse(&serde_json::to_vec(&entry).unwrap())
        .unwrap()
        .connect()
        .unwrap();
    let start = Start::new(
        "guide",
        Path::new("/home/sam/Agents/guide"),
        "claude",
        Some("opus"),
        Some("acme/app"),
    )
    .unwrap();
    let id = "019a7c105c407a108b203c40d5e6f708";
    let first = connection.start_first_agent(&start, id).unwrap();
    assert!(first.ok && first.name == "guide" && first.ready == Readiness::NotWaited);
    let second = connection.start_first_agent(&start, id).unwrap();
    assert!(!second.ok && second.ready == Readiness::Exited);
    assert_eq!(
        connection.start_first_agent(&start, id).err(),
        Some(AddError::Refused("Directory does not exist: /x".into()))
    );
    // A badly spelled request ID is refused before anything is sent.
    assert_eq!(
        connection
            .start_first_agent(&start, "019A7C10-5C40-7A10-8B20-3C40D5E6F708")
            .err(),
        Some(AddError::Connection(Error::Invalid))
    );
    let requests = server.join().unwrap();
    for request in &requests[1..] {
        assert!(request.starts_with("post /api/agents/start "), "{request}");
        assert!(request.contains(&format!("x-backbone-request-id: {id}\r\n")));
        let body: Value = serde_json::from_str(request.rsplit("\r\n\r\n").next().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"name": "guide", "dir": "/home/sam/agents/guide", "runtime": "claude",
                "model": "opus", "watch": ["acme/app"], "wait": false})
        );
    }
    // Reads still carry no request ID.
    assert!(!requests[0].contains("x-backbone-request-id"));
    for invalid in [
        Start::new("guide", Path::new("Agents/guide"), "claude", None, None),
        Start::new("guide", Path::new("/a"), "shell", None, None),
        Start::new("guide", Path::new("/a"), "claude", None, Some("acme")),
        Start::new("guide", Path::new("/a"), "claude", None, Some("acme/app/x")),
    ] {
        assert_eq!(invalid.err(), Some(Error::Invalid));
    }
}
