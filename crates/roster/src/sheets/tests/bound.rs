use super::*;
use crate::profiles::{Connection, Profile};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::mpsc,
    time::Instant,
};

fn connection(
    label: &str,
    script: impl FnOnce(UnixListener, serde_json::Value) + Send + 'static,
) -> (
    tempfile::TempDir,
    Profile,
    Connection,
    thread::JoinHandle<()>,
) {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let credential = directory.path().join("profile.env");
    fs::write(&credential, "BACKBONE_API_KEY=fixture-original\n").unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = json!({"schema_version":1,"profile_id":label,
        "installation_id":"installation","lifecycle_id":"lifecycle",
        "baseline_version":"fixture","tmux":{"socket_path":directory.path().join("tmux.sock")}});
    let profile = Profile::parse(
        &serde_json::to_vec(&json!({"schema_version":1,
        "profile_id":label,"display_name":label,"kind":"isolated","state":"ready",
        "binding":{"installation_id":"installation","lifecycle_id":"lifecycle",
            "endpoint":{"kind":"unix","path":url.strip_prefix("unix:").unwrap()},
            "credential_reference":{"kind":"dotenv","path":credential,"key":"BACKBONE_API_KEY"},
            "tmux":identity["tmux"]}}))
        .unwrap(),
    )
    .unwrap();
    let label = label.to_owned();
    let worker = thread::spawn(move || {
        let (mut stream, body) = next(&listener, &label, "fixture-original", "GET /api/profile");
        assert!(body.is_empty());
        reply(&mut stream, 200, identity.clone());
        script(listener, identity);
    });
    let connection = profile.connect().unwrap();
    (directory, profile, connection, worker)
}

fn next(listener: &UnixListener, label: &str, key: &str, path: &str) -> (UnixStream, Vec<u8>) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "missing request: {path}");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (headers, body) = request(&mut stream);
    assert!(
        headers.starts_with(&format!("{path} HTTP/1.1\r\n")),
        "{headers}"
    );
    let headers = headers.to_ascii_lowercase();
    for (header, value) in [
        ("authorization", format!("bearer {key}")),
        ("x-backbone-profile-id", label.into()),
        ("x-backbone-installation-id", "installation".into()),
        ("x-backbone-lifecycle-id", "lifecycle".into()),
    ] {
        assert!(
            headers.contains(&format!("{header}: {value}\r\n")),
            "{headers}"
        );
    }
    (stream, body)
}

fn feedback() -> Feedback {
    Feedback::operator(id(7), id(3), FeedbackKind::Feedback, "Updated".into(), None)
}

#[test]
fn delayed_sheets_and_prepared_retries_keep_original_profile_and_credentials() {
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_a, profile, connection_a, worker_a) = connection("a", move |listener, identity| {
        let (mut stream, _) = next(
            &listener,
            "a",
            "fixture-original",
            &format!("GET /api/sheets/{}", id(1)),
        );
        arrived.send(()).unwrap();
        delayed.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut a = record(8);
        a["title"] = json!("sentinel-a");
        reply(&mut stream, 200, a);
        let path = format!("POST /api/sheets/{}/feedback", id(1));
        let (stream, original) = next(&listener, "a", "fixture-original", &path);
        drop(stream);
        let (mut stream, _) = next(&listener, "a", "fixture-rotated", "GET /api/profile");
        reply(&mut stream, 200, identity);
        let (mut stream, retried) = next(&listener, "a", "fixture-original", &path);
        assert_eq!(original, retried);
        reply(&mut stream, 200, receipt("sentinel-a"));
        let (mut stream, _) = next(
            &listener,
            "a",
            "fixture-original",
            &format!(
                "GET /api/sheets/{}/export?revision_id={}&format=json",
                id(1),
                id(8)
            ),
        );
        reply(&mut stream, 200, json!({"sentinel":"export-a"}));
    });
    let (_b, _, connection_b, worker_b) = connection("b", |listener, _| {
        let (mut stream, _) = next(
            &listener,
            "b",
            "fixture-original",
            &format!("GET /api/sheets/{}", id(1)),
        );
        let mut b = record(8);
        b["title"] = json!("sentinel-b");
        reply(&mut stream, 200, b);
    });
    let a = Client::bound(connection_a.clone());
    let b = Client::bound(connection_b);
    let pending_read = {
        let a = a.clone();
        thread::spawn(move || a.record(&id(1), None).unwrap())
    };
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(b.record(&id(1), None).unwrap().title, "sentinel-b");
    release.send(()).unwrap();
    assert_eq!(pending_read.join().unwrap().title, "sentinel-a");
    let prepared = a.prepare_feedback(&id(1), &feedback()).unwrap();
    assert!(matches!(b.submit(&prepared), Err(Error::ProviderChanged)));
    assert!(matches!(a.submit(&prepared), Err(Error::Unknown)));
    fs::write(
        connection_a.credential_reference(),
        "BACKBONE_API_KEY=fixture-rotated\n",
    )
    .unwrap();
    let rotated = Client::bound(profile.connect().unwrap());
    assert_eq!(rotated.identity(), a.identity());
    assert!(
        matches!(rotated.submit(&prepared), Ok(Accepted::Feedback(r)) if r.operation_id == "sentinel-a")
    );
    let exported = a.export(&id(1), &id(8), Format::Json, None).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&exported.bytes).unwrap(),
        json!({"sentinel":"export-a"})
    );
    assert!(!format!("{a:?} {prepared:?}").contains("fixture-original"));
    worker_a.join().unwrap();
    worker_b.join().unwrap();
}

#[test]
fn binding_refusals_stop_pages_and_mutations_without_retry_or_fallback() {
    let (_directory, _, connection, worker) = connection("a", |listener, _| {
        let (mut stream, _) = next(
            &listener,
            "a",
            "fixture-original",
            "GET /api/sheets?limit=50",
        );
        reply(
            &mut stream,
            200,
            json!({"items":[record(8)],"next_cursor":"second"}),
        );
        let (mut stream, _) = next(
            &listener,
            "a",
            "fixture-original",
            "GET /api/sheets?limit=50&cursor=second",
        );
        reply(
            &mut stream,
            409,
            json!({"detail":{"code":"binding_mismatch"}}),
        );
        let (mut stream, _) = next(
            &listener,
            "a",
            "fixture-original",
            &format!("POST /api/sheets/{}/feedback", id(1)),
        );
        reply(
            &mut stream,
            409,
            json!({"detail":{"code":"binding_required"}}),
        );
    });
    let client = Client::bound(connection);
    assert_eq!(client.list_all().unwrap_err(), Error::BindingRefused);
    let prepared = client.prepare_feedback(&id(1), &feedback()).unwrap();
    assert_eq!(client.submit(&prepared).unwrap_err(), Error::BindingRefused);
    worker.join().unwrap();
}
