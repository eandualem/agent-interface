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
    thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut requests = Vec::new();
        for (status, body) in replies {
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
            requests.push(String::from_utf8(header).unwrap().to_ascii_lowercase());
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
