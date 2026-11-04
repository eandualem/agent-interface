use super::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    thread,
    time::Instant,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/profiles-v1.json")).unwrap()
}

fn inspected(registration: Value) -> Value {
    json!({"name":"worker","known":true,"runtime":"codex","registration_id":registration,"recent_deliveries":[{"kind":"direct_message","outcome":"queued","preview":"fixture","created_at":"2026-11-02T07:00:00Z"}]})
}

#[test]
fn retained_runtime_context_uses_canonical_permissions_and_original_registration() {
    use super::runtime_context::{self as context, Status};
    let fixtures = fixture();
    let context_fixtures = &fixtures["runtime_context"];
    let registration = context_fixtures["available"]["registration_id"].clone();
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let mut replies = vec![(200, identity), (200, inspected(registration.clone()))];
    for name in ["available", "missing", "invalid", "unreadable"] {
        replies.push((200, context_fixtures[name].clone()));
    }
    // Unsupported metadata must still match the captured runtime.
    let mut unsupported = context_fixtures["unsupported"].clone();
    unsupported["runtime"] = json!("codex");
    replies.push((200, unsupported));
    replies.push((200, inspected(registration.clone())));
    replies.push((
        409,
        context_fixtures["registration_changed"]["body"].clone(),
    ));
    replies.push((
        409,
        fixtures["http_errors"]["binding_mismatch"]["body"].clone(),
    ));
    replies.push((404, json!({})));
    let server = serve(listener, replies);
    let connection = profile.connect().unwrap();
    let card = context::inspect(connection.clone(), "worker", None).unwrap();
    assert_eq!(
        card.target.registration_id(),
        registration.as_str().unwrap()
    );
    assert!(card.agent.permissions.files.is_empty());
    assert_eq!(card.agent.received.len(), 1);
    assert_eq!(card.agent.received[0].preview, "fixture");
    fs::write(
        connection.credential_reference(),
        "BACKBONE_API_KEY=rotated\n",
    )
    .unwrap();
    for expected in [
        Status::Available,
        Status::Unavailable,
        Status::Unavailable,
        Status::Unavailable,
        Status::Unsupported,
    ] {
        let permissions = card.target.permissions().unwrap();
        assert_eq!(permissions.status, expected);
        assert_eq!(
            permissions.message().is_none(),
            expected == Status::Available
        );
    }
    assert_eq!(card.target.refresh().unwrap().name, "worker");
    assert_eq!(
        card.target.refresh().err(),
        Some(context::Error::RegistrationChanged)
    );
    assert_eq!(
        card.target.permissions().err(),
        Some(context::Error::Connection(Error::BindingMismatch))
    );
    assert_eq!(
        card.target.permissions().err(),
        Some(context::Error::Unsupported)
    );
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("get /api/agents/worker/inspect http/1.1"));
    for request in &requests[2..] {
        assert!(request.lines().next().unwrap().contains(&format!(
            "?expected_registration_id={} ",
            registration.as_str().unwrap()
        )));
        assert!(request.contains("authorization: bearer fixture-original\r\n"));
        assert!(request.contains("x-backbone-profile-id: profile-a\r\n"));
        assert!(request.contains("x-backbone-installation-id: installation-a\r\n"));
        assert!(request.contains("x-backbone-lifecycle-id: lifecycle-a1\r\n"));
    }
}

#[test]
fn runtime_context_rejects_foreign_targets_and_malformed_permission_metadata() {
    use super::runtime_context as context;
    let fixtures = fixture();
    let available = &fixtures["runtime_context"]["available"];
    let registration = available["registration_id"].as_str().unwrap();
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let mut replies = vec![(200, identity), (200, inspected(json!(registration)))];
    let mut expected = Vec::new();
    for field in [
        "profile_id",
        "installation_id",
        "lifecycle_id",
        "agent_name",
        "runtime",
        "registration_id",
    ] {
        let mut wrong = available.clone();
        wrong[field] = json!("other");
        replies.push((200, wrong));
        expected.push(if field == "registration_id" {
            context::Error::RegistrationChanged
        } else {
            context::Error::Invalid
        });
    }
    for path in [
        "/private/config.toml",
        "../config.toml",
        "project/../../config.toml",
        "C:\\config.toml",
    ] {
        let mut wrong = available.clone();
        wrong["permissions"]["files"][0]["display_path"] = json!(path);
        replies.push((200, wrong));
        expected.push(context::Error::Invalid);
    }
    for field in ["allow", "deny", "ask", "modes", "files"] {
        let mut wrong = available.clone();
        wrong["permissions"][field] = json!("malformed");
        replies.push((200, wrong));
        expected.push(context::Error::Invalid);
    }
    for field in ["scope", "state"] {
        let mut wrong = available.clone();
        wrong["permissions"]["files"][0][field] = json!("unknown");
        replies.push((200, wrong));
        expected.push(context::Error::Invalid);
    }
    let mut invalid_available = available.clone();
    invalid_available["permissions"]["files"][0]["state"] = json!("unreadable");
    replies.push((200, invalid_available));
    expected.push(context::Error::Invalid);
    let mut raw_error = fixtures["runtime_context"]["missing"].clone();
    raw_error["permissions"]["error"]["message"] = json!("private provider diagnostic");
    replies.push((200, raw_error));
    expected.push(context::Error::Invalid);
    replies.push((200, json!({"schema_version":2,"replacement":true})));
    expected.push(context::Error::Unsupported);
    let server = serve(listener, replies);
    let card = context::inspect(profile.connect().unwrap(), "worker", Some(registration)).unwrap();
    for error in expected {
        assert_eq!(card.target.permissions().err(), Some(error));
    }
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with(&format!(
        "get /api/agents/worker/inspect?expected_registration_id={registration} "
    )));
}

#[test]
fn retained_inspection_cannot_replace_or_drop_registration() {
    use super::runtime_context as context;
    let fixtures = fixture();
    let registration = fixtures["runtime_context"]["available"]["registration_id"]
        .as_str()
        .unwrap();
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
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
        let (directory, listener, url) = crate::transport::tests::local_listener();
        let (mut profile, mut identity) =
            profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
        profile.item.profile_id = format!("profile-{label}");
        identity["profile_id"] = json!(profile.item.profile_id);
        let mut permissions = fixture()["runtime_context"]["available"].clone();
        permissions["profile_id"] = json!(profile.item.profile_id);
        permissions["permissions"]["allow"] = json!([format!("rule-{label}")]);
        let registration = permissions["registration_id"].clone();
        let server = serve(
            listener,
            vec![
                (200, identity),
                (200, inspected(registration)),
                (200, permissions),
            ],
        );
        let card = context::inspect(profile.connect().unwrap(), "worker", None).unwrap();
        profiles.push((directory, server, card));
    }
    let (_b_dir, b_server, b) = profiles.pop().unwrap();
    let (_a_dir, a_server, a) = profiles.pop().unwrap();
    assert_eq!(b.target.permissions().unwrap().allow, ["rule-b"]);
    assert_eq!(a.target.permissions().unwrap().allow, ["rule-a"]);
    for (server, label) in [(a_server, "a"), (b_server, "b")] {
        for request in server.join().unwrap() {
            assert!(request.contains(&format!("x-backbone-profile-id: profile-{label}\r\n")));
        }
    }
}

#[test]
fn canonical_discovery_decodes_and_invalid_bindings_are_not_selectable() {
    let fixture = fixture();
    let bytes = |value: &Value| serde_json::to_vec(value).unwrap();
    let future = br#"{"schema_version":2,"new_shape":true}"#;
    assert_eq!(list(future).err(), Some(Error::Unsupported));
    assert_eq!(Profile::parse(future).err(), Some(Error::Unsupported));
    let items = list(&bytes(&fixture["discovery"]["list"])).unwrap();
    assert_eq!(items.len(), 1);
    let ready = Profile::parse(&bytes(&fixture["discovery"]["show_ready"])).unwrap();
    assert_eq!(ready.item().profile_id, items[0].profile_id);
    let creating = Profile::parse(&bytes(&fixture["discovery"]["show_creating"])).unwrap();
    assert_eq!(creating.connect().err(), Some(Error::NotReady));

    let mut invalid = fixture["discovery"]["show_ready"].clone();
    invalid["schema_version"] = json!(2);
    assert_eq!(
        Profile::parse(&bytes(&invalid)).err(),
        Some(Error::Unsupported)
    );
    invalid["schema_version"] = json!(1);
    invalid["binding"] = Value::Null;
    assert_eq!(Profile::parse(&bytes(&invalid)).err(), Some(Error::Invalid));
    invalid.as_object_mut().unwrap().remove("binding");
    assert_eq!(Profile::parse(&bytes(&invalid)).err(), Some(Error::Invalid));
    invalid = fixture["discovery"]["show_ready"].clone();
    invalid["binding"]["endpoint"]["kind"] = json!("tcp");
    assert_eq!(Profile::parse(&bytes(&invalid)).err(), Some(Error::Invalid));
    invalid = fixture["discovery"]["show_ready"].clone();
    invalid["binding"]["credential_reference"]["path"] = json!("relative/.env");
    assert_eq!(Profile::parse(&bytes(&invalid)).err(), Some(Error::Invalid));
}

fn profile_at(directory: &Path, socket: &str) -> (Profile, Value) {
    let mut fixture = fixture();
    let credential = directory.join("profile.env");
    fs::write(&credential, "BACKBONE_API_KEY=fixture-original\n").unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let tmux = directory.join("tmux.sock");
    let record = &mut fixture["discovery"]["show_ready"];
    record["binding"]["endpoint"]["path"] = json!(socket);
    record["binding"]["credential_reference"]["path"] = json!(credential);
    record["binding"]["tmux"]["socket_path"] = json!(tmux);
    let profile = Profile::parse(&serde_json::to_vec(record).unwrap()).unwrap();
    fixture["identity"]["tmux"]["socket_path"] = json!(tmux);
    (profile, fixture["identity"].clone())
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
fn connection_pins_credentials_and_identity_through_followup_and_replacement() {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let mismatch = fixture()["http_errors"]["binding_mismatch"]["body"].clone();
    let server = serve(
        listener,
        vec![(200, identity), (200, json!({})), (409, mismatch)],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(connection.identity().lifecycle_id, "lifecycle-a1");
    assert_eq!(connection.tmux_socket(), directory.path().join("tmux.sock"));
    fs::write(
        connection.credential_reference(),
        "BACKBONE_API_KEY=fixture-rotated\n",
    )
    .unwrap();
    let response = connection
        .response("/api/example", Some(vec![]), 1024)
        .unwrap();
    assert_eq!(response.status, 200);
    // The same path now refuses the old instance. No unbound retry follows.
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
        assert!(request.contains("authorization: bearer fixture-original\r\n"));
        assert!(request.contains("x-backbone-profile-id: profile-a\r\n"));
        assert!(request.contains("x-backbone-installation-id: installation-a\r\n"));
        assert!(request.contains("x-backbone-lifecycle-id: lifecycle-a1\r\n"));
    }
}

#[test]
fn bound_shell_request_carries_no_environment_and_checks_its_reply() {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let reply = |shell_id: &str, lifecycle: &str| {
        json!({"schema_version": 1, "profile_id": identity["profile_id"],
            "installation_id": identity["installation_id"], "lifecycle_id": lifecycle,
            "shell_id": shell_id, "session": "opaque-session", "created": true})
    };
    let lifecycle = identity["lifecycle_id"].as_str().unwrap();
    let server = serve(
        listener,
        vec![
            (200, identity.clone()),
            (200, reply("123456abcdef", lifecycle)),
            (200, reply("123456abcdef", "lifecycle-other")),
            (200, reply("foreign000000", lifecycle)),
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
    assert!(requests[1].contains("x-backbone-lifecycle-id: "));
    let body = requests[1].rsplit("\r\n\r\n").next().unwrap();
    let keys: Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        keys,
        json!({"schema_version": 1, "shell_id": "123456abcdef", "cwd": "/work"})
    );
}

#[test]
fn shared_session_cases_decode_and_refusals_keep_their_status() {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let sessions = fixture()["sessions"].clone();
    let tuple = |mut reply: Value| {
        for key in ["profile_id", "installation_id", "lifecycle_id"] {
            reply[key] = identity[key].clone();
        }
        reply
    };
    let refusal = |case: &Value| {
        (
            case["status"].as_u64().unwrap() as u16,
            case["body"].clone(),
        )
    };
    let mut foreign = tuple(sessions["sign_in_response"].clone());
    foreign["lifecycle_id"] = json!("lifecycle-other");
    let server = serve(
        listener,
        vec![
            (200, identity.clone()),
            (200, tuple(sessions["shell_response"].clone())),
            (200, tuple(sessions["shell_reused"].clone())),
            refusal(&sessions["shell_refusals"]["invalid_cwd"]),
            refusal(&sessions["shell_refusals"]["shell_unavailable"]),
            (200, tuple(sessions["sign_in_response"].clone())),
            (200, tuple(sessions["sign_in_reused"].clone())),
            refusal(&sessions["sign_in_refusals"]["runtime_unavailable"]),
            refusal(&sessions["sign_in_refusals"]["shell_unavailable"]),
            (200, foreign),
            (404, json!({})),
        ],
    );
    let connection = profile.connect().unwrap();
    let shell_id = sessions["shell_request"]["shell_id"].as_str().unwrap();
    let shell = |cwd| connection.ensure_shell(shell_id, cwd);
    let session = sessions["shell_response"]["session"].as_str().unwrap();
    assert_eq!(shell(Some("/work")).as_deref(), Ok(session));
    assert_eq!(shell(None).as_deref(), Ok(session));
    assert_eq!(shell(None).err(), Some(Error::Refused(422)));
    assert_eq!(shell(None).err(), Some(Error::Refused(503)));

    let runtime = sessions["sign_in_request"]["runtime"].as_str().unwrap();
    let session = sessions["sign_in_response"]["session"].as_str().unwrap();
    let sign_in = || connection.sign_in_session(runtime);
    assert_eq!(sign_in().as_deref(), Ok(session));
    assert_eq!(sign_in().as_deref(), Ok(session));
    assert_eq!(sign_in().err(), Some(Error::Refused(422)));
    assert_eq!(sign_in().err(), Some(Error::Refused(503)));
    assert_eq!(sign_in().err(), Some(Error::BindingMismatch));
    assert_eq!(sign_in().err(), Some(Error::Unsupported));

    let requests = server.join().unwrap();
    assert!(requests[5].starts_with("post /api/profile/accounts/sign-in "));
    let body = requests[5].rsplit("\r\n\r\n").next().unwrap();
    let sent: Value = serde_json::from_str(body).unwrap();
    assert_eq!(sent, sessions["sign_in_request"]);
}

#[test]
fn bound_roster_preserves_typed_failures_and_decodes_success() {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let required = fixture()["http_errors"]["binding_required"]["body"].clone();
    let server = serve(
        listener,
        vec![
            (200, identity),
            (409, required),
            (403, json!({})),
            (200, json!({"items": "invalid"})),
            (200, json!({"items": [{"name": "worker", "state": "busy"}]})),
            (200, json!({"items": []})),
        ],
    );
    let connection = profile.connect().unwrap();
    assert_eq!(connection.roster().err(), Some(Error::BindingRequired));
    assert_eq!(connection.roster().err(), Some(Error::Refused(403)));
    assert_eq!(connection.roster().err(), Some(Error::Invalid));
    let roster = connection.roster().unwrap();
    assert_eq!(roster.states["worker"], "busy");
    assert!(roster.error.is_none() && roster.observed_at.is_some());
    assert!(connection.roster().unwrap().states.is_empty());
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 6);
    for request in &requests[1..] {
        assert!(request.starts_with("get /api/agents "));
        assert!(request.contains("x-backbone-lifecycle-id: lifecycle-a1\r\n"));
    }
}

#[test]
fn handshake_rejects_wrong_identity_tmux_and_unsupported_provider() {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let (profile, mut identity) = profile_at(directory.path(), url.strip_prefix("unix:").unwrap());
    let mut wrong_instance = identity.clone();
    wrong_instance["lifecycle_id"] = json!("lifecycle-a2");
    identity["tmux"]["socket_path"] = json!(directory.path().join("other.sock"));
    let required = fixture()["http_errors"]["binding_required"]["body"].clone();
    let server = serve(
        listener,
        vec![
            (200, wrong_instance),
            (200, identity),
            (404, json!({})),
            (409, required),
            (200, json!({"schema_version":2,"new_shape":true})),
        ],
    );
    assert_eq!(profile.connect().err(), Some(Error::BindingMismatch));
    assert_eq!(profile.connect().err(), Some(Error::BindingMismatch));
    assert_eq!(profile.connect().err(), Some(Error::Unsupported));
    assert_eq!(profile.connect().err(), Some(Error::BindingRequired));
    assert_eq!(profile.connect().err(), Some(Error::Unsupported));
    assert_eq!(server.join().unwrap().len(), 5);
}

#[test]
fn credential_source_must_be_the_exact_private_regular_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.env");
    fs::write(&path, "BACKBONE_API_KEY=fixture-key\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(credential(&path).unwrap(), "fixture-key");
    let alias = directory.path().join("alias.env");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert_eq!(credential(&alias), Err(Error::Unavailable));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(credential(&path), Err(Error::Unavailable));
    assert_eq!(credential(directory.path()), Err(Error::Unavailable));
}

mod library_snapshots;
