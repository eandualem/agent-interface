use super::*;
use agent_interface_roster::{profiles, views::ReportQuery};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::mpsc,
    time::Instant,
};

pub(crate) struct Reply {
    pub(crate) path: String,
    pub(crate) prefix: bool,
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) delay: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    pub(crate) replace: bool,
    pub(crate) post: bool,
}

impl Reply {
    pub(crate) fn ok(path: &str, body: Value) -> Self {
        Self {
            path: path.into(),
            prefix: false,
            status: 200,
            body,
            delay: None,
            replace: false,
            post: false,
        }
    }

    pub(crate) fn post(path: &str, status: u16, body: Value) -> Self {
        Self {
            status,
            post: true,
            ..Self::ok(path, body)
        }
    }

    fn refusal(path: &str, code: &str) -> Self {
        Self {
            status: 409,
            replace: true,
            ..Self::ok(path, json!({"detail":{"code":code}}))
        }
    }
}

// Scripted disposable providers assert every path, credential and identity header.
pub(crate) fn provider(
    label: &str,
    replies: Vec<Reply>,
) -> (tempfile::TempDir, Connection, JoinHandle<()>) {
    provider_with_tmux(label, replies, None)
}

pub(crate) fn provider_with_tmux(
    label: &str,
    replies: Vec<Reply>,
    tmux: Option<std::path::PathBuf>,
) -> (tempfile::TempDir, Connection, JoinHandle<()>) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.path().join("api.sock");
    let mut listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let credential = directory.path().join("profile.env");
    fs::write(&credential, format!("BACKBONE_API_KEY=fixture-{label}\n")).unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = json!({
        "schema_version":1, "profile_id":format!("profile-{label}"),
        "installation_id":format!("installation-{label}"), "lifecycle_id":format!("lifecycle-{label}"),
        "baseline_version":"fixture", "tmux":{"socket_path":tmux.unwrap_or_else(|| directory.path().join("tmux.sock"))}
    });
    let profile = profiles::Profile::parse(&serde_json::to_vec(&json!({
        "schema_version":1,"profile_id":identity["profile_id"],"display_name":label,
        "kind":"isolated","state":"ready","binding":{
            "installation_id":identity["installation_id"],"lifecycle_id":identity["lifecycle_id"],
            "endpoint":{"kind":"unix","path":socket},
            "credential_reference":{"kind":"dotenv","path":credential,"key":"BACKBONE_API_KEY"},
            "tmux":identity["tmux"]
        }
    })).unwrap()).unwrap();
    let label = label.to_owned();
    let server = thread::spawn(move || {
        let mut replies = std::iter::once(Reply::ok("/api/profile", identity))
            .chain(replies)
            .peekable();
        while let Some(reply) = replies.next() {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "no request for {}", reply.path);
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
            let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            if let Some(length) = request
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
            {
                let mut body = vec![0; length.trim().parse().unwrap()];
                stream.read_exact(&mut body).unwrap();
            }
            if reply.prefix {
                assert!(
                    request.starts_with(&format!("get {}", reply.path)),
                    "{request}"
                );
                assert!(request.lines().next().unwrap().ends_with(" http/1.1"));
            } else {
                let method = if reply.post { "post" } else { "get" };
                assert!(
                    request.starts_with(&format!("{method} {} http/1.1\r\n", reply.path)),
                    "{request}"
                );
            }
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
                    request.contains(&format!("{header}: {value}\r\n")),
                    "{request}"
                );
            }
            if let Some((arrived, release)) = reply.delay {
                arrived.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            // Complete replacement before releasing the preceding response,
            // so the client cannot race a socket unlink/rebind gap.
            if replies.peek().is_some_and(|next| next.replace) {
                drop(listener);
                fs::remove_file(&socket).unwrap();
                listener = UnixListener::bind(&socket).unwrap();
                fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
            }
            let body = reply.body.to_string();
            write!(
                stream,
                "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                reply.status,
                body.len()
            )
            .unwrap();
        }
    });
    let connection = profile.connect().unwrap();
    (directory, connection, server)
}

pub(crate) fn record(label: &str) -> Value {
    let section = json!({"text":format!("sentinel-{label}"),"links":[]});
    json!({"id":7,"author_id":"b".repeat(32),"author_name":"worker","agent_name":"worker",
        "created_at":"2026-11-02T10:00:00Z","conversation_id":"7",
        "report":{"status":"active","goal":section,"progress":section,"blockers":section,"next":section},
        "response":{"question":"Question","answer":format!("sentinel-{label}"),"action":"Action"}})
}

fn document(label: &str) -> Value {
    let html = format!("<p>sentinel-{label}</p>");
    json!({"id":7,"document_id":"a".repeat(32),"author_id":"b".repeat(32),
        "author_name":"worker","agent_name":"worker","title":format!("sentinel-{label}"),
        "created_at":"2026-11-02T10:00:00Z","content_bytes":html.len(),
        "content_sha256":format!("{:x}",Sha256::digest(html.as_bytes())),
        "html":html,"supersedes_id":null,"superseded_by":null})
}

fn roster(label: &str) -> Reply {
    Reply::ok(
        "/api/agents",
        json!({"items":[{"name":"worker","runtime":format!("sentinel-{label}")}]}),
    )
}

#[test]
fn same_ids_keep_content_pagination_details_and_credentials_on_each_origin() {
    let feeds = [
        (
            ReportQuery::Latest(None),
            "/api/reports?limit=20&history=false",
            "&cursor=next",
        ),
        (
            ReportQuery::Recent(None),
            "/api/reports?limit=20&history=true",
            "&cursor=next",
        ),
        (
            ReportQuery::Responses(None),
            "/api/responses?limit=20",
            "&before_id=7",
        ),
        (
            ReportQuery::Documents(None, None),
            "/api/documents?limit=20",
            "&before_id=7",
        ),
    ];
    let replies = |label| {
        let mut replies = Vec::new();
        for (query, path, cursor) in &feeds {
            let item = match query {
                ReportQuery::Latest(_) | ReportQuery::Recent(_) => json!({"record":record(label)}),
                ReportQuery::Responses(_) => record(label),
                ReportQuery::Documents(..) => document(label),
            };
            replies.push(Reply::ok(
                &format!("{path}&agent=worker"),
                json!({"items":[item],"has_more":true,"next_cursor":"next","next_before_id":7}),
            ));
            replies.push(roster(label));
            replies.push(Reply::ok(
                &format!("{path}{cursor}&agent=worker"),
                json!({"items":[item],"has_more":false}),
            ));
            replies.push(roster(label));
        }
        replies.extend([
            Reply::ok("/api/reports/7", record(label)),
            Reply::ok("/api/responses/7", record(label)),
            Reply::ok("/api/documents/7", document(label)),
        ]);
        replies
    };
    let (_dir_a, a, server_a) = provider("a", replies("a"));
    let (_dir_b, b, server_b) = provider("b", replies("b"));
    fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    for (query, _, _) in feeds {
        for (connection, label) in [(&a, "a"), (&b, "b")] {
            let request = content::Request::Reports(query.clone(), Some("worker".into()));
            let first = content::read(connection, &request, &|| true);
            assert_eq!(&first.identity, connection.identity());
            assert_eq!(first.request, request);
            let content::Data::Reports(page) = first.result.unwrap() else {
                panic!()
            };
            assert!(page.items[0].provider_url.is_empty());
            assert_eq!(
                page.runtimes.get("worker").map(String::as_str),
                (!matches!(query, ReportQuery::Documents(..)))
                    .then_some(format!("sentinel-{label}").as_str())
            );
            assert!(format!("{:?}", page.items).contains(&format!("sentinel-{label}")));
            let request = content::Request::Reports(page.next.unwrap(), Some("worker".into()));
            let content::Data::Reports(next) = content::read(connection, &request, &|| true)
                .result
                .unwrap()
            else {
                panic!()
            };
            assert!(next.next.is_none());
            assert!(format!("{:?}", next.items).contains(&format!("sentinel-{label}")));
        }
    }
    for request in [
        content::Request::Report(7),
        content::Request::Response(7),
        content::Request::Document(7),
    ] {
        for (connection, label) in [(&a, "a"), (&b, "b")] {
            let read = content::read(connection, &request, &|| true);
            assert_eq!(&read.identity, connection.identity());
            assert!(format!("{:?}", read.result.unwrap()).contains(&format!("sentinel-{label}")));
        }
    }
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn replacement_refusals_survive_optional_enrichment_and_document_fetch() {
    let (_dir, connection, server) = provider(
        "a",
        vec![
            Reply::ok(
                "/api/reports?limit=20&history=true",
                json!({"items":[],"has_more":false}),
            ),
            Reply::refusal("/api/agents", "binding_mismatch"),
            Reply::ok(
                "/api/documents?limit=20",
                json!({"items":[],"has_more":false}),
            ),
            Reply::refusal("/api/agents", "binding_required"),
            Reply::refusal("/api/documents/7", "binding_mismatch"),
            Reply {
                status: 404,
                ..Reply::ok("/api/documents/7", json!({}))
            },
        ],
    );
    for (request, error) in [
        (
            content::Request::Reports(ReportQuery::Recent(None), None),
            profiles::Error::BindingMismatch,
        ),
        (
            content::Request::Reports(ReportQuery::Documents(None, None), None),
            profiles::Error::BindingRequired,
        ),
        (
            content::Request::Document(7),
            profiles::Error::BindingMismatch,
        ),
    ] {
        let read = content::read(&connection, &request, &|| true);
        assert_eq!(read.identity, *connection.identity());
        assert_eq!(read.result, Err(content::Error::Connection(error)));
    }
    assert_eq!(
        content::read(&connection, &content::Request::Document(7), &|| true).result,
        Err(content::Error::Content(
            "This document revision is no longer available from Agent Backbone."
        ))
    );
    server.join().unwrap();
}

#[test]
fn delayed_workers_cancel_old_generations_and_clear_pending_results() {
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let request = content::Request::Reports(ReportQuery::Documents(None, None), None);
    let list = json!({"items":[document("a")],"has_more":false});
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Reply {
                delay: Some((arrived, delayed)),
                ..Reply::ok("/api/documents?limit=20", list.clone())
            },
            // The cancelled chain must not request roster enrichment.
            Reply::ok("/api/documents?limit=20", list),
            roster("a"),
            Reply::ok("/api/documents/7", document("a")),
        ],
    );
    let (_dir_b, b, server_b) = provider("b", vec![Reply::ok("/api/responses/7", record("b"))]);
    let identity_a = a.identity().clone();
    let identity_b = b.identity().clone();
    let (wake_a, woken_a) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let source_a =
        BoundContentSource::start(a, request.clone(), Duration::from_millis(10), move || {
            wake_a.send(()).unwrap();
            resumed.recv_timeout(Duration::from_secs(5)).unwrap();
        })
        .unwrap();
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    let (wake_b, woken_b) = mpsc::channel();
    let source_b = BoundContentSource::start(
        b,
        content::Request::Response(7),
        Duration::from_secs(60),
        move || {
            wake_b.send(()).unwrap();
        },
    )
    .unwrap();
    woken_b.recv_timeout(Duration::from_secs(5)).unwrap();
    let observed_b = source_b.take_update().unwrap();
    assert_eq!(observed_b.read.identity, identity_b);
    assert!(format!("{:?}", observed_b.read.result).contains("sentinel-b"));
    source_a.set_request(content::Request::Response(7));
    let generation = source_a.set_request(request);
    release.send(()).unwrap();
    woken_a.recv_timeout(Duration::from_secs(5)).unwrap();
    {
        let state = source_a.state.0.lock().unwrap();
        let update = state.update.as_ref().unwrap();
        assert_eq!(update.generation, generation);
        assert_eq!(update.read.identity, identity_a);
        assert!(update.read.result.is_ok());
    }
    // A completed but unread result is also invalidated by a new selection.
    source_a.set_request(content::Request::Document(7));
    assert!(source_a.take_update().is_none());
    source_a.set_active(false);
    resume.send(()).unwrap();
    assert!(woken_a.recv_timeout(Duration::from_millis(60)).is_err());
    let generation = source_a.set_active(true);
    woken_a.recv_timeout(Duration::from_secs(5)).unwrap();
    let update = source_a.take_update().unwrap();
    assert_eq!(update.generation, generation);
    assert_eq!(update.read.identity, identity_a);
    assert!(format!("{:?}", update.read.result).contains("sentinel-a"));
    resume.send(()).unwrap();
    // A Document ignores its short polling interval; B retains its own wait.
    assert!(woken_a.recv_timeout(Duration::from_millis(60)).is_err());
    assert!(woken_b.recv_timeout(Duration::from_millis(60)).is_err());
    drop(source_a);
    drop(source_b);
    server_a.join().unwrap();
    server_b.join().unwrap();
}
