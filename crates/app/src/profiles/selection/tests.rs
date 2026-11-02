use super::*;
use crate::roster::content::tests::provider;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn identity(label: &str, directory: &Path) -> Value {
    json!({"schema_version":1,"profile_id":format!("profile-{label}"),
        "installation_id":format!("installation-{label}"),"lifecycle_id":format!("lifecycle-{label}"),
        "baseline_version":"fixture","tmux":{"socket_path":directory.join("tmux.sock")}})
}
fn client(directory: &Path, connection: &Connection) -> Client {
    let record = json!({"schema_version":1,"profile_id":connection.identity().profile_id,
        "display_name":"same label","kind":"isolated","state":"ready","binding":{
            "installation_id":connection.identity().installation_id,"lifecycle_id":connection.identity().lifecycle_id,
            "endpoint":{"kind":"unix","path":directory.join("api.sock")},
            "credential_reference":{"kind":"dotenv","path":connection.credential_reference(),"key":"BACKBONE_API_KEY"},
            "tmux":{"socket_path":connection.tmux_socket()}}});
    fs::write(
        directory.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let executable = directory.join("provider");
    fs::write(&executable,"#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CALLS\"\n[ \"$1 $2 $3 $4\" = 'profiles show --json --' ] || exit 9\n/bin/cat \"$RECORD\"\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    client.environment.push((
        "RECORD".into(),
        directory.join("record.json").into_os_string(),
    ));
    client
        .environment
        .push(("CALLS".into(), directory.join("calls").into_os_string()));
    client
}
fn fixture(
    label: &str,
    delay: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    mismatch: bool,
) -> (
    tempfile::TempDir,
    Connection,
    Client,
    thread::JoinHandle<()>,
) {
    let (directory, connection, initial) = provider(label, vec![]);
    initial.join().unwrap();
    let client = client(directory.path(), &connection);
    let path = directory.path().join("api.sock");
    fs::remove_file(&path).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut body = identity(label, directory.path());
    if mismatch {
        body["lifecycle_id"] = json!("replacement");
    }
    let label = label.to_owned();
    let server = thread::spawn(move || {
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
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 8192);
        }
        let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /api/profile http/1.1\r\n"));
        for expected in [
            format!("x-backbone-profile-id: profile-{label}"),
            format!("x-backbone-installation-id: installation-{label}"),
            format!("x-backbone-lifecycle-id: lifecycle-{label}"),
            format!("authorization: bearer fixture-{label}"),
        ] {
            assert!(request.contains(&expected), "missing {expected}");
        }
        if let Some((arrived, held)) = delay {
            arrived.send(()).unwrap();
            held.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let body = serde_json::to_vec(&body).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    });
    (directory, connection, client, server)
}

#[test]
fn delayed_selection_cannot_replace_a_later_verified_choice_or_existing_connection() {
    let (arrived, waiting) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let (dir_a, a, client_a, server_a) = fixture("a", Some((arrived, held)), false);
    let (_dir_b, b, client_b, server_b) = fixture("b", None, false);
    let mut selection = Selection::default();
    let old = selection.begin(&client_a, "profile-a").unwrap();
    let job = thread::spawn(move || old.run());
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    let next = selection.begin(&client_b, "profile-b").unwrap();
    let accepted = selection.accept(next.run()).unwrap().unwrap();
    release.send(()).unwrap();
    assert!(selection.accept(job.join().unwrap()).is_none());
    assert_eq!(accepted.connection().identity(), b.identity());
    assert_eq!(accepted.item().profile_id, "profile-b");
    assert_eq!(accepted.item().display_name, "same label");
    assert_eq!(a.identity().profile_id, "profile-a");
    assert!(selection.pending().is_none());
    assert_eq!(
        fs::read_to_string(dir_a.path().join("calls")).unwrap(),
        "profiles show --json -- profile-a\n"
    );
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn repeating_same_choice_and_dropping_picker_cancel_obsolete_work_before_execution() {
    let (directory, _, client, server) = fixture("a", None, false);
    let mut selection = Selection::default();
    let first = selection.begin(&client, "profile-a").unwrap();
    let second = selection.begin(&client, "profile-a").unwrap();
    assert!(selection.accept(first.run()).is_none());
    assert_eq!(selection.pending(), Some("profile-a"));
    assert!(selection.accept(second.run()).unwrap().is_ok());
    assert!(selection.pending().is_none());
    server.join().unwrap();
    let discarded = selection.begin(&client, "profile-a").unwrap();
    drop(selection);
    assert_eq!(discarded.run().result.err(), Some(Error::Cancelled));
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "profiles show --json -- profile-a\n"
    );
}

#[test]
fn registry_readiness_and_identity_refusals_never_yield_selection_authority() {
    let (directory, _, client, server) = fixture("a", None, true);
    let mut selection = Selection::default();
    let attempt = selection.begin(&client, "profile-a").unwrap();
    assert_eq!(
        selection.accept(attempt.run()).unwrap().err(),
        Some(Error::Profile(
            agent_interface_roster::profiles::Error::BindingMismatch
        ))
    );
    server.join().unwrap();
    let mut record: Value =
        serde_json::from_slice(&fs::read(directory.path().join("record.json")).unwrap()).unwrap();
    record["state"] = json!("creating");
    record["binding"] = Value::Null;
    fs::write(
        directory.path().join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let attempt = selection.begin(&client, "profile-a").unwrap();
    assert_eq!(
        selection.accept(attempt.run()).unwrap().err(),
        Some(Error::Profile(
            agent_interface_roster::profiles::Error::NotReady
        ))
    );
    let foreign = selection.begin(&client, "profile-b").unwrap();
    assert_eq!(
        selection.accept(foreign.run()).unwrap().err(),
        Some(Error::Profile(
            agent_interface_roster::profiles::Error::BindingMismatch
        ))
    );
    let cancelled = selection.begin(&client, "profile-a").unwrap();
    selection.cancel();
    assert!(selection.accept(cancelled.run()).is_none());
    assert!(selection.begin(&client, "bad id").is_err());
    assert_eq!(
        fs::read_to_string(directory.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}
