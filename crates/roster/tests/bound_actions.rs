//! Bound action clients keep reads and mutations on their original profile.

use agent_interface_roster::{
    conversations::{self, Client, Kind, Message, Page},
    feedback,
    profiles::{Connection, Error, Profile},
    views::{ReportContent, ReportItem},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Step {
    path: &'static str,
    request: Option<Value>,
    reply: Option<(u16, Value)>,
    delay: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    replace: bool,
}

impl Step {
    fn get(path: &'static str, reply: Value) -> Self {
        Self {
            path,
            request: None,
            reply: Some((200, reply)),
            delay: None,
            replace: false,
        }
    }
    fn post(path: &'static str, request: Value, reply: Value) -> Self {
        Self {
            request: Some(request),
            ..Self::get(path, reply)
        }
    }
}

fn provider(label: &str, steps: Vec<Step>) -> (tempfile::TempDir, Connection, JoinHandle<()>) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("api.sock");
    let mut listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let credential = directory.path().join("profile.env");
    fs::write(&credential, format!("BACKBONE_API_KEY=fixture-{label}\n")).unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = json!({"schema_version":1,"profile_id":format!("profile-{label}"),
        "installation_id":format!("installation-{label}"),"lifecycle_id":format!("lifecycle-{label}"),
        "baseline_version":"fixture","tmux":{"socket_path":directory.path().join("tmux.sock")}});
    let profile = Profile::parse(&serde_json::to_vec(&json!({"schema_version":1,
        "profile_id":identity["profile_id"],"display_name":label,"kind":"isolated","state":"ready",
        "binding":{"installation_id":identity["installation_id"],"lifecycle_id":identity["lifecycle_id"],
            "endpoint":{"kind":"unix","path":path},
            "credential_reference":{"kind":"dotenv","path":credential,"key":"BACKBONE_API_KEY"},
            "tmux":identity["tmux"]}})).unwrap()).unwrap();
    let label = label.to_owned();
    let server = thread::spawn(move || {
        let mut steps = std::iter::once(Step::get("/api/profile", identity))
            .chain(steps)
            .peekable();
        while let Some(step) = steps.next() {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "no request for {}", step.path);
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                assert!(bytes.len() < 8192);
            }
            let headers = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            let method = if step.request.is_some() {
                "post"
            } else {
                "get"
            };
            assert!(
                headers.starts_with(&format!("{method} {} http/1.1\r\n", step.path)),
                "{headers}"
            );
            for (header, value) in [
                ("authorization", format!("bearer fixture-{label}")),
                ("x-backbone-profile-id", format!("profile-{label}")),
                (
                    "x-backbone-installation-id",
                    format!("installation-{label}"),
                ),
                ("x-backbone-lifecycle-id", format!("lifecycle-{label}")),
            ] {
                assert!(
                    headers.contains(&format!("{header}: {value}\r\n")),
                    "{headers}"
                );
            }
            if let Some(expected) = step.request {
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse::<usize>()
                    .unwrap();
                assert!(length < 8192);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
            }
            if let Some((arrived, release)) = step.delay {
                arrived.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            // Publish the replacement before the client receives the preceding reply.
            if steps.peek().is_some_and(|next| next.replace) {
                drop(listener);
                fs::remove_file(&path).unwrap();
                listener = UnixListener::bind(&path).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            if let Some((status, body)) = step.reply {
                let body = body.to_string();
                write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        }
    });
    let connection = profile.connect().unwrap();
    (directory, connection, server)
}

fn entry(label: &str) -> Value {
    json!({"id":7,"kind":"message","text":format!("sentinel-{label}"),"created_at":"2026-11-02T10:00:00Z","author":{"type":"owner"}})
}
fn page(label: &str) -> Value {
    json!({"kind":"report","artifact_id":"7","responsible":{"agent_id":"worker-id","name":"worker","status":"active"},
        "total":1,"unread":1,"entries":[entry(label)],"older":"older"})
}
fn accepted(label: &str) -> Value {
    json!({"result":"accepted","entry":entry(label)})
}

#[test]
fn conversations_keep_delayed_sends_retries_pagination_and_markers_on_original_binding() {
    let message = Message {
        request_id: "same-request".into(),
        text: "Draft".into(),
        recipient: Some("worker-id".into()),
        anchor: None,
    };
    let body = serde_json::to_value(&message).unwrap();
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Step::get("/api/conversations/report/7?limit=50", page("a")),
            Step {
                reply: None,
                delay: Some((arrived, delayed)),
                ..Step::post(
                    "/api/conversations/report/7/messages",
                    body.clone(),
                    json!({}),
                )
            },
            Step::post(
                "/api/conversations/report/7/messages",
                body.clone(),
                accepted("a"),
            ),
            Step::post(
                "/api/conversations/report/7/read",
                json!({"read_through":7}),
                json!({"read_through":7,"total":1,"unread":0}),
            ),
            Step::post(
                "/api/conversations/report/7/messages/7/redeliver",
                json!({"request_id":"retry-delivery"}),
                json!({"result":"already_pending"}),
            ),
            Step::get(
                "/api/conversations/report/7?limit=50&before=older",
                page("a"),
            ),
            Step {
                reply: Some((409, json!({"detail":{"code":"binding_mismatch"}}))),
                replace: true,
                ..Step::post(
                    "/api/conversations/report/7/messages",
                    body.clone(),
                    json!({}),
                )
            },
        ],
    );
    let (_dir_b, b, server_b) = provider(
        "b",
        vec![
            Step::get("/api/conversations/report/7?limit=50", page("b")),
            Step::post("/api/conversations/report/7/messages", body, accepted("b")),
            Step::get(
                "/api/conversations/report/7?limit=50&before=older",
                page("b"),
            ),
        ],
    );
    assert!(matches!(
        Client::new(a.clone(), b.identity(), Kind::Report, "7".into()),
        Err(conversations::Error::Connection(Error::BindingMismatch))
    ));
    let client_a = Client::new(a.clone(), a.identity(), Kind::Report, "7".into()).unwrap();
    let client_b = Client::new(b.clone(), b.identity(), Kind::Report, "7".into()).unwrap();
    let first = client_a.show(&Page::Newest).unwrap();
    assert!(format!("{:?}", first.entries).contains("sentinel-a"));
    let pending = client_a.clone();
    let draft = message.clone();
    let sender = thread::spawn(move || pending.send(&draft));
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    let second = client_b.show(&Page::Newest).unwrap();
    assert!(format!("{:?}", second.entries).contains("sentinel-b"));
    assert!(matches!(
        client_b.send(&message),
        conversations::Outcome::Accepted(_)
    ));
    release.send(()).unwrap();
    assert_eq!(sender.join().unwrap(), conversations::Outcome::Unknown);
    assert!(matches!(
        client_a.send(&message),
        conversations::Outcome::Accepted(_)
    ));
    assert_eq!(client_a.read(7).unwrap().read_through, 7);
    assert_eq!(
        client_a.redeliver(7, "retry-delivery"),
        conversations::Outcome::AlreadyPending
    );
    assert!(
        format!(
            "{:?}",
            client_a
                .show(&Page::Before(first.older.unwrap()))
                .unwrap()
                .entries
        )
        .contains("sentinel-a")
    );
    assert!(
        matches!(client_a.send(&message),conversations::Outcome::Refused{error,..} if error=="binding_refused")
    );
    assert!(
        format!(
            "{:?}",
            client_b
                .show(&Page::Before(second.older.unwrap()))
                .unwrap()
                .entries
        )
        .contains("sentinel-b")
    );
    assert_eq!(client_a.identity(), a.identity());
    assert_eq!(client_b.identity(), b.identity());
    server_a.join().unwrap();
    server_b.join().unwrap();
}

fn item() -> ReportItem {
    ReportItem {
        id: 7,
        author: "worker".into(),
        author_id: Some("b".repeat(32)),
        registered: true,
        provider_url: String::new(),
        created_at: "2026-11-02T10:00:00Z".into(),
        conversation_id: Some("7".into()),
        content: ReportContent::Response {
            question: "Question".into(),
            answer: "Answer".into(),
            action: "Action".into(),
            superseded_by: None,
        },
    }
}

#[test]
fn feedback_pins_author_verification_and_submission_and_preserves_unknown_delivery() {
    let author = json!({"id":7,"author_id":"b".repeat(32),"agent_name":"worker"});
    let body = json!({"target_session":"worker","from_entity":"agent-interface-app",
        "message":"Feedback on response #7 (source=/api/responses/7).\n\nFeedback","priority":false});
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Step::get("/api/responses/7", author.clone()),
            Step {
                reply: None,
                ..Step::post("/api/messages", body.clone(), json!({}))
            },
            Step::get(
                "/api/responses/7",
                json!({"id":7,"author_id":"c".repeat(32),"agent_name":"worker"}),
            ),
            Step::get("/api/responses/7", author.clone()),
            Step {
                reply: Some((409, json!({"detail":{"code":"binding_required"}}))),
                replace: true,
                ..Step::post("/api/messages", body.clone(), json!({}))
            },
        ],
    );
    let (_dir_b, b, server_b) = provider(
        "b",
        vec![
            Step::get("/api/responses/7", author),
            Step::post(
                "/api/messages",
                body,
                json!({"ok":true,"queued":false,"outcome":"delivered","detail":"sentinel-b"}),
            ),
        ],
    );
    assert!(matches!(
        feedback::Client::new(a.clone(), b.identity(), item()),
        Err(Error::BindingMismatch)
    ));
    let client_a = feedback::Client::new(a.clone(), a.identity(), item()).unwrap();
    let client_b = feedback::Client::new(b.clone(), b.identity(), item()).unwrap();
    fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    assert!(matches!(
        client_a.send("Feedback"),
        feedback::Outcome::Unknown(_)
    ));
    assert!(
        matches!(client_a.send("Feedback"),feedback::Outcome::Refused(reason) if reason.contains("author changed"))
    );
    assert!(
        matches!(client_a.send("Feedback"),feedback::Outcome::Refused(reason) if reason.contains("binding was refused"))
    );
    assert_eq!(
        client_b.send("Feedback"),
        feedback::Outcome::Accepted("sentinel-b".into())
    );
    assert_eq!(client_a.identity(), a.identity());
    assert_eq!(client_b.identity(), b.identity());
    server_a.join().unwrap();
    server_b.join().unwrap();
}

fn plan_page(label: &str, next: Option<&str>) -> Value {
    json!({"items":[{"author_id":"11111111111111111111111111111111","agent_name":label,"record":null}],"next_after_author_id":next})
}

#[test]
fn plans_keep_delayed_pages_and_rotated_credentials_on_the_original_binding() {
    use agent_interface_roster::plans;
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Step {
                delay: Some((arrived, delayed)),
                ..Step::get(
                    "/api/current-plans?limit=50",
                    plan_page("a", Some("11111111111111111111111111111111")),
                )
            },
            Step::get(
                "/api/current-plans?limit=50&after_author_id=11111111111111111111111111111111",
                plan_page("a", None),
            ),
            Step::get(
                "/api/current-plans?limit=50",
                plan_page("a", Some("11111111111111111111111111111111")),
            ),
            Step {
                reply: Some((409, json!({"detail":{"code":"binding_mismatch"}}))),
                replace: true,
                ..Step::get(
                    "/api/current-plans?limit=50&after_author_id=11111111111111111111111111111111",
                    json!({}),
                )
            },
        ],
    );
    let (_dir_b, b, server_b) = provider(
        "b",
        vec![Step::get(
            "/api/current-plans?limit=50",
            plan_page("b", None),
        )],
    );
    let a = plans::Client::new(a);
    let b = plans::Client::new(b);
    assert_ne!(a.identity(), b.identity());
    let worker = thread::spawn({
        let a = a.clone();
        move || a.list_all()
    });
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(b.list_all().unwrap().items[0].agent_name, "b");
    fs::write(
        _dir_a.path().join("profile.env"),
        "BACKBONE_API_KEY=rotated\n",
    )
    .unwrap();
    release.send(()).unwrap();
    let pages = worker.join().unwrap().unwrap();
    assert_eq!(pages.items.len(), 2);
    assert!(pages.items.iter().all(|entry| entry.agent_name == "a"));
    assert_eq!(
        a.list_all(),
        Err(plans::Error::Connection(Error::BindingMismatch))
    );
    assert_eq!(a.list(Some("bad cursor")), Err(plans::Error::Unreadable));
    server_a.join().unwrap();
    server_b.join().unwrap();
}

fn interaction_fixture(name: &str) -> Value {
    let bytes: &[u8] = match name {
        "summary" => include_bytes!("fixtures/interactions/summary.json"),
        "history" => include_bytes!("fixtures/interactions/history.json"),
        "detail" => include_bytes!("fixtures/interactions/detail.json"),
        "states" => include_bytes!("fixtures/interactions/state-spans.json"),
        _ => unreachable!(),
    };
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn interactions_retain_snapshot_credentials_for_delayed_history_and_detail_pages() {
    use agent_interface_roster::interactions;
    let summary_path = "/api/interactions/summary?since=start&until=end";
    let history_path = "/api/interactions?since=start&until=end&agent_id=worker&direction=both&limit=200&snapshot=fixture-not-a-live-cursor";
    let state_path =
        "/api/interactions/state-spans?since=start&until=end&limit=500&agent_id=worker";
    let detail_path = "/api/interactions/11111111111111111111111111111111";
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let mut history_a = interaction_fixture("history");
    history_a["items"][0]["body"]["text"] = json!("sentinel-a");
    let mut detail_a = interaction_fixture("detail");
    detail_a["body"]["text"] = json!("sentinel-a");
    detail_a["attempts"]["next_cursor"] = json!("more");
    detail_a["lifecycle"]["next_cursor"] = json!("more");
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Step {
                delay: Some((arrived, delayed)),
                ..Step::get(summary_path, interaction_fixture("summary"))
            },
            Step::get(history_path, history_a),
            Step::get(state_path, interaction_fixture("states")),
            Step::get(detail_path, detail_a),
            Step::get(
                "/api/interactions/11111111111111111111111111111111/attempts?limit=200&cursor=more",
                json!({"items":[],"next_cursor":null}),
            ),
            Step::get(
                "/api/interactions/11111111111111111111111111111111/events?limit=200&cursor=more",
                json!({"items":[],"next_cursor":null}),
            ),
            Step {
                reply: Some((409, json!({"detail":{"code":"binding_mismatch"}}))),
                replace: true,
                ..Step::get(detail_path, json!({}))
            },
        ],
    );
    let (_dir_b, b, server_b) = provider(
        "b",
        vec![
            Step::get(summary_path, interaction_fixture("summary")),
            Step::get(history_path, interaction_fixture("history")),
            Step::get(state_path, interaction_fixture("states")),
            Step::get(detail_path, interaction_fixture("detail")),
        ],
    );
    let a = interactions::Client::new(a);
    let b = interactions::Client::new(b);
    let origin = a.identity().clone();
    let worker = thread::spawn(move || a.read("start", "end", Some("worker")));
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    let b = b.read("start", "end", Some("worker")).unwrap();
    assert_ne!(b.identity(), Some(&origin));
    assert_ne!(
        b.detail("11111111111111111111111111111111")
            .unwrap()
            .body
            .text
            .as_deref(),
        Some("sentinel-a")
    );
    fs::write(
        _dir_a.path().join("profile.env"),
        "BACKBONE_API_KEY=rotated\n",
    )
    .unwrap();
    release.send(()).unwrap();
    let a = worker.join().unwrap().unwrap();
    assert_eq!(a.identity(), Some(&origin));
    assert!(a.provider.is_empty());
    assert_eq!(
        a.history.as_ref().unwrap().items[0].body.text.as_deref(),
        Some("sentinel-a")
    );
    assert_eq!(
        a.detail("11111111111111111111111111111111")
            .unwrap()
            .body
            .text
            .as_deref(),
        Some("sentinel-a")
    );
    assert_eq!(
        a.detail("11111111111111111111111111111111"),
        Err(interactions::Error::Connection(Error::BindingMismatch))
    );
    assert!(!format!("{a:?}").contains("fixture-a"));
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn interaction_state_binding_refusal_invalidates_the_whole_snapshot() {
    use agent_interface_roster::interactions;
    let summary_path = "/api/interactions/summary?since=start&until=end";
    let state_path = "/api/interactions/state-spans?since=start&until=end&limit=500";
    let (_dir, connection, server) = provider(
        "a",
        vec![
            Step::get(summary_path, interaction_fixture("summary")),
            Step {
                reply: Some((404, json!({}))),
                ..Step::get(state_path, json!({}))
            },
            Step::get(summary_path, interaction_fixture("summary")),
            Step {
                reply: Some((409, json!({"detail":{"code":"binding_required"}}))),
                replace: true,
                ..Step::get(state_path, json!({}))
            },
        ],
    );
    let client = interactions::Client::new(connection);
    assert_eq!(
        client.read("start", "end", None).unwrap().state_error,
        Some(interactions::Error::Unsupported)
    );
    assert_eq!(
        client.read("start", "end", None).unwrap_err(),
        interactions::Error::Connection(Error::BindingRequired)
    );
    server.join().unwrap();
}
