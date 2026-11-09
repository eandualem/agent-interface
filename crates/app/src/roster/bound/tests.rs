use super::*;
use agent_interface_roster::profiles::Profile;
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    time::Instant,
};

// Two requests after the handshake: a sentinel roster, then either another
// observation or a refusal from a replacement bound at the same socket path.
fn provider(
    label: &'static str,
    delay: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    replace: bool,
) -> (tempfile::TempDir, Profile, JoinHandle<()>) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("api.sock");
    let mut listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let credential = directory.path().join("profile.env");
    fs::write(&credential, format!("BACKBONE_API_KEY=fixture-{label}\n")).unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let tmux = directory.path().join("tmux.sock");
    let profile = Profile::parse(
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "profile_id": format!("profile-{label}"),
            "display_name": label,
            "kind": "isolated",
            "state": "ready",
            "binding": {
                "installation_id": format!("installation-{label}"),
                "lifecycle_id": format!("lifecycle-{label}"),
                "endpoint": {"kind": "unix", "path": path},
                "credential_reference": {"kind": "dotenv", "path": credential, "key": "BACKBONE_API_KEY"},
                "tmux": {"socket_path": tmux}
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let server = thread::spawn(move || {
        for attempt in 0..3 {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fixture received no request");
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
            let target = if attempt == 0 {
                "/api/profile"
            } else {
                "/api/agents"
            };
            assert!(request.starts_with(&format!("get {target} http/1.1\r\n")));
            for (header, value) in [
                ("authorization", format!("bearer fixture-{label}")),
                ("x-backbone-profile-id", format!("profile-{label}")),
                (
                    "x-backbone-installation-id",
                    format!("installation-{label}"),
                ),
                ("x-backbone-lifecycle-id", format!("lifecycle-{label}")),
            ] {
                assert!(request.contains(&format!("{header}: {value}\r\n")));
            }
            if attempt == 1
                && let Some((arrived, release)) = &delay
            {
                arrived.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            let (status, body) = if attempt == 0 {
                (
                    200,
                    json!({
                        "schema_version": 1,
                        "profile_id": format!("profile-{label}"),
                        "installation_id": format!("installation-{label}"),
                        "lifecycle_id": format!("lifecycle-{label}"),
                        "baseline_version": "fixture",
                        "tmux": {"socket_path": tmux}
                    }),
                )
            } else if attempt == 2 && replace {
                (
                    409,
                    json!({"detail": {"code": "binding_mismatch", "message": "Profile binding does not match."}}),
                )
            } else {
                (
                    200,
                    json!({"items": [{
                        "name": "worker", "state": "busy", "reason": format!("sentinel-{label}-{attempt}")
                    }]}),
                )
            };
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            if attempt == 1 && replace {
                drop(listener);
                fs::remove_file(&path).unwrap();
                listener = UnixListener::bind(&path).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    });
    (directory, profile, server)
}

#[test]
fn bound_workers_keep_delayed_responses_credentials_and_stale_caches_separate() {
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_directory_a, profile_a, server_a) = provider("a", Some((arrived, delayed)), true);
    let (_directory_b, profile_b, server_b) = provider("b", None, false);
    let connection_a = profile_a.connect().unwrap();
    let connection_b = profile_b.connect().unwrap();
    let credential_a = connection_a.credential_reference().to_owned();
    let identity_a = connection_a.identity().clone();
    let identity_b = connection_b.identity().clone();
    let (wake_a, woken_a) = mpsc::channel();
    let (resume_a, resumed_a) = mpsc::channel();
    let source_a = BoundRosterSource::start(connection_a, move || {
        let _ = wake_a.send(());
        // Hold each published observation until the test has inspected it.
        assert_ne!(
            resumed_a.recv_timeout(Duration::from_secs(10)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    })
    .unwrap();
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    // A is in flight while B completes; changing A's credential locator cannot
    // change the credential used by either existing worker on its next poll.
    fs::write(credential_a, "BACKBONE_API_KEY=fixture-rotated\n").unwrap();
    let (wake_b, woken_b) = mpsc::channel();
    let (resume_b, resumed_b) = mpsc::channel();
    let source_b = BoundRosterSource::start(connection_b, move || {
        let _ = wake_b.send(());
        assert_ne!(
            resumed_b.recv_timeout(Duration::from_secs(10)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    })
    .unwrap();
    woken_b.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(source_a.take_update().is_none());
    release.send(()).unwrap();
    woken_a.recv_timeout(Duration::from_secs(5)).unwrap();
    let first_a = source_a.updates.lock().unwrap().as_ref().unwrap().clone();
    assert_eq!(first_a.identity, identity_a);
    assert_eq!(
        first_a.snapshot.details["worker"].reason.as_deref(),
        Some("sentinel-a-1")
    );
    assert!(first_a.error.is_none() && first_a.snapshot.error.is_none());
    let first_b = source_b.take_update().unwrap();
    assert_eq!(first_b.identity, identity_b);
    assert_eq!(
        first_b.snapshot.details["worker"].reason.as_deref(),
        Some("sentinel-b-1")
    );
    assert!(first_b.error.is_none() && first_b.snapshot.error.is_none());
    // A's refusal must replace its unread success, retaining A's timestamp and
    // sentinel. Meanwhile B continues polling its own provider successfully.
    resume_a.send(()).unwrap();
    resume_b.send(()).unwrap();
    woken_a.recv_timeout(Duration::from_secs(5)).unwrap();
    let failed_a = source_a.take_update().unwrap();
    assert_eq!(failed_a.identity, identity_a);
    assert_eq!(failed_a.error, Some(Error::BindingMismatch));
    assert!(failed_a.snapshot.error.is_some());
    assert_eq!(failed_a.snapshot.observed_at, first_a.snapshot.observed_at);
    assert_eq!(failed_a.snapshot.states, first_a.snapshot.states);
    assert_eq!(failed_a.snapshot.details, first_a.snapshot.details);
    woken_b.recv_timeout(Duration::from_secs(5)).unwrap();
    let next_b = source_b.take_update().unwrap();
    assert_eq!(next_b.identity, identity_b);
    assert_eq!(
        next_b.snapshot.details["worker"].reason.as_deref(),
        Some("sentinel-b-2")
    );
    assert!(next_b.error.is_none() && next_b.snapshot.error.is_none());
    assert!(next_b.snapshot.observed_at > first_b.snapshot.observed_at);
    // Stop before releasing the final callbacks, so a third poll cannot race
    // teardown after the fixture providers have finished serving responses.
    source_a.stop.send(()).unwrap();
    source_b.stop.send(()).unwrap();
    drop(resume_a);
    drop(resume_b);
    drop(source_a);
    drop(source_b);
    assert_eq!(woken_a.recv(), Err(mpsc::RecvError));
    assert_eq!(woken_b.recv(), Err(mpsc::RecvError));
    server_a.join().unwrap();
    server_b.join().unwrap();
}
