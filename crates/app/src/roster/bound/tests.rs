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
    let tmux = directory.path().join("tmux.sock");
    let profile = Profile::parse(
        &serde_json::to_vec(&json!({
            "state": "serving", "protocol_version": 1,
            "profile_id": format!("profile-{label}"),
            "display_name": label,
            "instance_id": format!("instance-{label}"),
            "machine": {"id": format!("machine-{label}")},
            "socket": path, "tmux_socket": tmux,
            "credential_reference": {"type": "socket"}
        }))
        .unwrap(),
    )
    .unwrap();
    let server = thread::spawn(move || {
        let mut attempt = 0;
        while attempt < 3 {
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
            // This provider has no push stream, and says so without counting.
            if request.starts_with("get /api/stream") {
                write!(
                    stream,
                    "HTTP/1.1 404 Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                continue;
            }
            // The swarm read after each roster is answered without counting,
            // and names the same profile as every request.
            let swarms = request.starts_with("get /api/swarms ");
            let target = if swarms {
                "/api/swarms"
            } else if attempt == 0 {
                "/api/profile"
            } else {
                "/api/agents"
            };
            assert!(request.starts_with(&format!("get {target} http/1.1\r\n")));
            assert!(request.contains(&format!("x-backbone-profile-id: profile-{label}\r\n")));
            if swarms {
                write!(stream, "HTTP/1.1 200 Fixture\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{{\"items\":[]}}").unwrap();
                continue;
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
                        "protocol_version": 1,
                        "profile_id": format!("profile-{label}"),
                        "instance_id": format!("instance-{label}"),
                        "machine": {"id": format!("machine-{label}")}
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
            attempt += 1;
        }
    });
    (directory, profile, server)
}

#[test]
fn bound_workers_keep_delayed_responses_and_stale_caches_separate() {
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_directory_a, profile_a, server_a) = provider("a", Some((arrived, delayed)), true);
    let (_directory_b, profile_b, server_b) = provider("b", None, false);
    let connection_a = profile_a.connect().unwrap();
    let connection_b = profile_b.connect().unwrap();
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
    // A is in flight while B completes.
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
    source_a.stop.send(true).unwrap();
    source_b.stop.send(true).unwrap();
    drop(resume_a);
    drop(resume_b);
    drop(source_a);
    drop(source_b);
    assert_eq!(woken_a.recv(), Err(mpsc::RecvError));
    assert_eq!(woken_b.recv(), Err(mpsc::RecvError));
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn a_refusal_stands_until_a_read_succeeds() {
    let refused = Error::Refused(401);
    assert_eq!(standing(Some(refused), Error::Unavailable), refused);
    assert_eq!(standing(None, Error::Unavailable), Error::Unavailable);
    assert_eq!(
        standing(Some(refused), Error::BindingMismatch),
        Error::BindingMismatch
    );
}

#[test]
fn pushed_state_and_run_changes_read_the_roster_and_runs_at_once() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = json!({
        "protocol_version": 1, "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true
    });
    let mut entry = identity.clone();
    entry["state"] = json!("serving");
    entry["socket"] = json!(path);
    entry["tmux_socket"] = json!(directory.path().join("tmux.sock"));
    entry["credential_reference"] = json!({"type": "socket"});
    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    let runs = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/src/profiles/runs/fixtures/runs.json"
    ));
    let (done, finished) = mpsc::channel::<()>();
    // The test acknowledges each restart step it read, so the next one is
    // pushed only then and no two steps arrive in one read.
    let (acknowledge, acknowledged) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        // A state change, a run change, then the steps of the agent's own
        // restart, each pushed once the read the one before it asked for
        // was answered.
        let pushes = [
            "agent.state",
            "run.changed",
            "agent.restart",
            "agent.restart",
            "agent.restart",
        ];
        let mut steps_read = 0;
        let mut stream = None::<std::os::unix::net::UnixStream>;
        let mut pushed: Vec<Instant> = Vec::new();
        let mut quiet = Instant::now();
        // How long after each push the roster was read.
        let mut reads_after_push = Vec::new();
        let mut runs_before_their_push = false;
        while finished.try_recv() == Err(mpsc::TryRecvError::Empty) {
            let mut socket = match listener.accept() {
                Ok((socket, _)) => socket,
                Err(_) => {
                    steps_read += acknowledged.try_iter().count();
                    // Push once the poller is waiting out its two seconds.
                    if let Some(open) = &mut stream
                        && pushed.len() < pushes.len()
                        && reads_after_push.len() == pushed.len()
                        && steps_read + 2 >= pushed.len()
                        && quiet.elapsed() > Duration::from_millis(300)
                    {
                        let kind = pushes[pushed.len()];
                        write!(open, "id: a:2\nevent: {kind}\ndata: {{}}\n\n").unwrap();
                        pushed.push(Instant::now());
                    }
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
            };
            socket.set_nonblocking(false).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            if request.starts_with(
                "GET /api/stream?kinds=agent.state,agent.changed,agent.restart,run.changed,resync ",
            ) {
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\nevent: hello\ndata: {{}}\n\n"
                )
                .unwrap();
                stream = Some(socket);
                continue;
            }
            let body = if request.starts_with("GET /api/profile ") {
                identity.to_string()
            } else if request.starts_with("GET /api/runs ") {
                runs_before_their_push |= pushed.len() < 2;
                runs.to_owned()
            } else {
                if request.starts_with("GET /api/agents ") && reads_after_push.len() < pushed.len()
                {
                    reads_after_push.push(pushed[reads_after_push.len()].elapsed());
                }
                // Background counts appear with the run change, so runs are
                // asked for only then.
                let state = if pushed.is_empty() { "idle" } else { "working" };
                let counts = (pushed.len() >= 2).then(|| json!({"running": 1, "queued": 1}));
                // The list carries the restart while it is under way: asked
                // for, stopping, then no more once it is ready.
                let restart = match pushed.len() {
                    3 => Some("waiting_for_turn"),
                    4 => Some("stopping"),
                    _ => None,
                }
                .map(|phase| json!({"id": 12, "phase": phase, "resume": false}));
                json!({"items": [{"name": "worker", "state": state, "background": counts,
                    "restart": restart}]})
                .to_string()
            };
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            quiet = Instant::now();
        }
        (reads_after_push, runs_before_their_push)
    });
    let (wake, woken) = mpsc::channel();
    let source = BoundRosterSource::start(profile.connect().unwrap(), move || {
        let _ = wake.send(());
    })
    .unwrap();
    // Each restart step as the list decodes it, once the runs are read.
    let steps = [Some("waiting_for_turn"), Some("stopping"), None];
    let mut read = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    while read < steps.len() {
        woken
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        // Two wakes can announce one update: the second then finds none.
        let Some(update) = source.take_update() else {
            continue;
        };
        if update.snapshot.states["worker"] != "working" || update.runs.active.len() != 2 {
            continue;
        }
        let restart = update.snapshot.details["worker"].restart.clone();
        let phase = restart
            .as_ref()
            .and_then(|restart| restart.phase.as_deref());
        // The step before the restart is listed reads as none too.
        if phase == steps[read] && (read > 0 || phase.is_some()) {
            if let Some(restart) = &restart {
                assert_eq!(restart.id, Some(12));
            }
            read += 1;
            let _ = acknowledge.send(());
        }
    }
    let dropped = Instant::now();
    drop(source);
    assert!(dropped.elapsed() < Duration::from_secs(1));
    done.send(()).unwrap();
    // Without the stream each read would come two seconds after the last.
    let (reads, runs_early) = server.join().unwrap();
    assert_eq!(reads.len(), 5, "{reads:?}");
    assert!(
        reads.iter().all(|read| *read < Duration::from_secs(1)),
        "{reads:?}"
    );
    // Without background counts or active runs, runs are never asked for.
    assert!(!runs_early);
}

#[test]
fn a_read_after_a_refusal_clears_it_and_shows_the_new_roster() {
    use crate::roster::content::tests::{Reply, provider};
    let roster = |state: &str, reason: &str| {
        Reply::ok(
            "/api/agents",
            json!({"items": [{"name": "worker", "state": state, "reason": reason}]}),
        )
    };
    let swarms = || Reply::ok("/api/swarms", json!({"items": []}));
    let (_directory, connection, server) = provider(
        "a",
        vec![
            roster("busy", "first"),
            swarms(),
            Reply {
                status: 401,
                ..Reply::ok("/api/agents", json!({}))
            },
            roster("waiting_for_human", "second"),
            swarms(),
        ],
    );
    let (wake, woken) = mpsc::channel();
    let source = BoundRosterSource::start(connection, move || {
        let _ = wake.send(());
    })
    .unwrap();
    let next = || loop {
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        if let Some(update) = source.take_update() {
            break update;
        }
    };
    let first = next();
    assert!(first.error.is_none() && first.snapshot.error.is_none());
    let refused = next();
    assert_eq!(refused.error, Some(Error::Refused(401)));
    assert!(refused.snapshot.error.is_some());
    assert_eq!(refused.snapshot.states, first.snapshot.states);
    assert_eq!(refused.snapshot.observed_at, first.snapshot.observed_at);
    let recovered = next();
    assert!(recovered.error.is_none() && recovered.snapshot.error.is_none());
    assert_eq!(recovered.snapshot.states["worker"], "waiting_for_human");
    assert_eq!(
        recovered.snapshot.details["worker"].reason.as_deref(),
        Some("second")
    );
    assert!(recovered.snapshot.observed_at > first.snapshot.observed_at);
    // Drop alone stops the worker, and its wake callback goes with it.
    drop(source);
    while woken.try_recv() == Ok(()) {}
    assert_eq!(woken.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    server.join().unwrap();
}
