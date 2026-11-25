use super::*;
use agent_interface_roster::profiles::Profile;
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    time::Instant,
};

/// The version check each connection makes first.
fn health() -> String {
    let version = agent_interface_roster::profiles::MINIMUM_BACKBONE.to_string();
    json!({ "version": version }).to_string()
}

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
            if request.starts_with("get /health ") {
                let body = health();
                write!(
                    stream,
                    "HTTP/1.1 200 Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                continue;
            }
            // The swarm and services reads after each roster are answered without counting,
            // and name the same profile as every request.
            let swarms = request.starts_with("get /api/swarms ");
            let services = request.starts_with("get /api/status/services ");
            let target = if swarms {
                "/api/swarms"
            } else if services {
                "/api/status/services"
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
            if services {
                write!(stream, "HTTP/1.1 200 Fixture\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{{\"machines\":[]}}").unwrap();
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
            let body = if request.starts_with("GET /health ") {
                health()
            } else if request.starts_with("GET /api/profile ") {
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
fn a_pushed_state_shows_before_a_read_and_outlives_a_read_it_overlaps() {
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
    let state = |state: &str| {
        format!(
            "event: agent.state\ndata: {}\n\n",
            json!({"agent": "worker", "registration_id": "r1", "state": state,
                "reason": null, "since": 1.0})
        )
    };
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (counted, (push, pushed)) = (Arc::clone(&reads), mpsc::channel::<String>());
    let (overlapped, overlap_done) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut stream = None::<std::os::unix::net::UnixStream>;
        loop {
            // A frame waits until the stream is open.
            if let Some(open) = &mut stream
                && let Ok(frame) = pushed.try_recv()
            {
                if frame.is_empty() {
                    break;
                }
                open.write_all(frame.as_bytes()).unwrap();
            }
            let Ok((mut socket, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            socket.set_nonblocking(false).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            if request.starts_with("GET /api/stream?") {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                    .unwrap();
                stream = Some(socket);
                continue;
            }
            let body = if request.starts_with("GET /health ") {
                health()
            } else if request.starts_with("GET /api/profile ") {
                identity.to_string()
            } else if request.starts_with("GET /api/agents ") {
                let read = counted.fetch_add(1, Ordering::SeqCst);
                // The second read is held open, then answered with what it saw
                // before the changes pushed while it was out.
                let held = Instant::now();
                while read == 1 && released.try_recv().is_err() {
                    // Frames pushed meanwhile still reach the stream.
                    assert!(held.elapsed() < Duration::from_secs(10), "never released");
                    if let Ok(frame) = pushed.try_recv() {
                        let open = stream.as_mut().unwrap();
                        open.write_all(frame.as_bytes()).unwrap();
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                let state = if read == 0 { "idle" } else { "busy" };
                json!({"items": [{"name": "worker", "registration_id": "r1", "state": state}]})
                    .to_string()
            } else {
                if request.starts_with("GET /api/status/services ")
                    && counted.load(Ordering::SeqCst) == 2
                {
                    let _ = overlapped.send(());
                }
                json!({"items": [], "machines": []}).to_string()
            };
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let (wake, woken) = mpsc::channel();
    let source = BoundRosterSource::start(profile.connect().unwrap(), move || {
        let _ = wake.send(());
    })
    .unwrap();
    let mut shown = None::<BoundRosterUpdate>;
    let mut show = |until: &dyn Fn(&BoundRosterUpdate) -> bool| {
        while !shown.as_ref().is_some_and(until) {
            woken.recv_timeout(Duration::from_secs(10)).unwrap();
            if let Some(update) = source.take_update() {
                shown = Some(update);
            }
        }
    };
    let state_is = |state: &'static str| {
        move |update: &BoundRosterUpdate| {
            update.snapshot.states.get("worker").map(String::as_str) == Some(state)
        }
    };
    show(&state_is("idle"));
    // A pushed state without its issue, from an older service, shows while
    // no later read can answer, and asks for a read behind it at once, not on
    // the two-second timer.
    push.send(state("busy")).unwrap();
    let asked = Instant::now();
    show(&state_is("busy"));
    while reads.load(Ordering::SeqCst) < 2 {
        assert!(asked.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(5));
    }
    // A wait pushed while that read is out outlives its older answer.
    let waits = json!({"agent": "worker", "registration_id": "r1",
        "waiting": {"on": "agent", "on_agent": "reviewer"},
        "background": {"running": 0, "queued": 0, "processes": 0}, "processes": []});
    push.send(format!("event: agent.changed\ndata: {waits}\n\n"))
        .unwrap();
    let waiting = |update: &BoundRosterUpdate| update.snapshot.details["worker"].waiting.is_some();
    show(&waiting);
    release.send(()).unwrap();
    overlap_done.recv_timeout(Duration::from_secs(10)).unwrap();
    thread::sleep(Duration::from_millis(300));
    while woken.try_recv().is_ok() {
        if let Some(update) = source.take_update() {
            shown = Some(update);
        }
    }
    let last = shown.unwrap();
    assert!(waiting(&last), "{:?}", last.snapshot.details["worker"]);
    assert_eq!(last.snapshot.states["worker"], "busy");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    // The service's frame with the agent's issue shows it and asks for no read.
    let mut fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/src/profiles/runs/fixtures/stream-agent-state.json"
    )))
    .unwrap();
    fixture["data"]["registration_id"] = json!("r1");
    push.send(format!(
        "event: {}\ndata: {}\n\n",
        fixture["event"].as_str().unwrap(),
        fixture["data"]
    ))
    .unwrap();
    loop {
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        if let Some(update) = source.take_update()
            && update.snapshot.details["worker"].current_issue == Some(42)
        {
            assert_eq!(update.snapshot.states["worker"], "idle");
            break;
        }
    }
    thread::sleep(Duration::from_millis(300));
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    // A registration the roster doesn't hold asks for a read.
    push.send(state("busy").replace("\"r1\"", "\"r9\""))
        .unwrap();
    let asked = Instant::now();
    while reads.load(Ordering::SeqCst) == 2 {
        assert!(asked.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(5));
    }
    drop(source);
    push.send(String::new()).unwrap();
    server.join().unwrap();
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
    let services = || Reply::ok("/api/status/services", json!({"machines": []}));
    let (_directory, connection, server) = provider(
        "a",
        vec![
            roster("busy", "first"),
            swarms(),
            services(),
            Reply {
                status: 401,
                ..Reply::ok("/api/agents", json!({}))
            },
            roster("waiting_for_human", "second"),
            swarms(),
            services(),
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

#[test]
fn machine_offline_and_recovery_updates_roster_live_without_restart() {
    use crate::roster::content::tests::{Reply, provider};
    use agent_interface_roster::MachineState;

    let remote_id = "f".repeat(64);
    let swarms = || Reply::ok("/api/swarms", json!({"items": []}));
    let (_directory, connection, server) = provider(
        "a",
        vec![
            Reply::ok(
                "/api/agents",
                json!({"items": [{
                    "name": "linux-worker", "state": "busy", "state_source": "push",
                    "machine": {"id": remote_id, "name": "build-linux"}
                }]}),
            ),
            swarms(),
            Reply::ok(
                "/api/status/services",
                json!({"machines": [
                    {"id": "machine-a", "name": "laptop", "home": true, "state": "online"},
                    {"id": remote_id, "name": "build-linux", "home": false, "state": "online"}
                ]}),
            ),
            Reply::ok(
                "/api/agents",
                json!({"items": [{
                    "name": "linux-worker", "state": "busy", "state_source": "stale",
                    "evidence": ["machine last reported 3m ago"],
                    "machine": {"id": remote_id, "name": "build-linux"}
                }]}),
            ),
            swarms(),
            Reply::ok(
                "/api/status/services",
                json!({"machines": [
                    {"id": "machine-a", "name": "laptop", "home": true, "state": "online"},
                    {"id": remote_id, "name": "build-linux", "home": false, "state": "offline",
                     "last_seen": "2026-11-22T12:00:00Z"}
                ]}),
            ),
            Reply::ok(
                "/api/agents",
                json!({"items": [{
                    "name": "linux-worker", "state": "idle", "state_source": "push",
                    "machine": {"id": remote_id, "name": "build-linux"}
                }]}),
            ),
            swarms(),
            Reply::ok(
                "/api/status/services",
                json!({"machines": [
                    {"id": "machine-a", "name": "laptop", "home": true, "state": "online"},
                    {"id": remote_id, "name": "build-linux", "home": false, "state": "online"}
                ]}),
            ),
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
    let online = next();
    assert_eq!(online.snapshot.machines[1].state, MachineState::Online);
    assert_eq!(
        online.snapshot.details["linux-worker"]
            .state_source
            .as_deref(),
        Some("push")
    );
    let offline = next();
    assert_eq!(offline.snapshot.machines[1].state, MachineState::Offline);
    assert_eq!(
        offline.snapshot.details["linux-worker"]
            .state_source
            .as_deref(),
        Some("stale")
    );
    let back = next();
    assert_eq!(back.snapshot.machines[1].state, MachineState::Online);
    assert_eq!(back.snapshot.states["linux-worker"], "idle");
    drop(source);
    server.join().unwrap();
}

#[test]
fn an_open_stream_slows_the_poll_and_its_end_reads_at_once() {
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
    let server = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut stream = None::<(std::os::unix::net::UnixStream, Instant)>;
        // When a resync was pushed, whether the roster read it asks for is to
        // fail, then the runs list read after it, then the machines read.
        let mut pushed = None::<Instant>;
        let mut fail = false;
        let mut fail_runs = false;
        let mut fail_machines = false;
        let mut closed = None::<Instant>;
        let mut reads = Vec::new();
        // Until two reads follow the stream's end.
        while closed.is_none_or(|closed| reads.iter().filter(|read| **read > closed).count() < 2) {
            assert!(Instant::now() < deadline, "{reads:?}");
            let mut socket = match listener.accept() {
                Ok((socket, _)) => socket,
                Err(_) => {
                    if let Some((open, opened)) = &mut stream {
                        // Once the read the hello asked for has landed and
                        // nothing else is being read: the first read always
                        // runs, and the hello's comes after it.
                        let last = reads.last().copied();
                        if pushed.is_none()
                            && reads.len() >= 2
                            && last.is_some_and(|last: Instant| {
                                last > *opened && last.elapsed() > Duration::from_millis(300)
                            })
                        {
                            write!(open, "id: a:2\nevent: resync\ndata: {{}}\n\n").unwrap();
                            pushed = Some(Instant::now());
                            fail = true;
                        }
                        // Open three seconds past the retry of the failed
                        // machines read, longer than a two-second poll.
                        let retried = pushed.and_then(|pushed| {
                            reads.iter().filter(|read| **read >= pushed).nth(3).copied()
                        });
                        if retried.is_some_and(|retried| retried.elapsed() > Duration::from_secs(3))
                        {
                            stream = None;
                            closed = Some(Instant::now());
                        }
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
            if request.starts_with("GET /api/stream") {
                // Opened once; asked again after its end, the service has none.
                if closed.is_none() {
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\nevent: hello\ndata: {{}}\n\n"
                    )
                    .unwrap();
                    stream = Some((socket, Instant::now()));
                } else {
                    write!(
                        socket,
                        "HTTP/1.1 404 Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                }
                continue;
            }
            let mut status = 200;
            let body = if request.starts_with("GET /health ") {
                health()
            } else if request.starts_with("GET /api/profile ") {
                identity.to_string()
            } else if request.starts_with("GET /api/swarms ") {
                json!({"items": []}).to_string()
            } else if request.starts_with("GET /api/status/services ") {
                if std::mem::take(&mut fail_machines) {
                    status = 503;
                }
                json!({"machines": []}).to_string()
            } else if request.starts_with("GET /api/runs ") {
                if std::mem::take(&mut fail_runs) {
                    status = 503;
                    fail_machines = true;
                }
                runs.to_owned()
            } else {
                assert!(request.starts_with("GET /api/agents "), "{request}");
                reads.push(Instant::now());
                if std::mem::take(&mut fail) {
                    status = 503;
                    fail_runs = true;
                }
                // A running agent, so the runs are listed after each roster.
                json!({"items": [{"name": "worker", "state": "working",
                    "background": {"running": 1, "queued": 0}}]})
                .to_string()
            };
            write!(
                socket,
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
        (pushed.unwrap(), closed.unwrap(), reads)
    });
    let source = BoundRosterSource::start(profile.connect().unwrap(), || {}).unwrap();
    let (pushed, closed, reads) = server.join().unwrap();
    drop(source);
    // While the stream is open the only reads after its hello are the failed
    // one the resync asked for, then a read two seconds after each: one whose
    // runs list fails, one whose machines read fails, and a good one.
    let open: Vec<_> = reads
        .iter()
        .filter(|read| **read >= pushed && **read < closed)
        .collect();
    assert_eq!(open.len(), 4, "{reads:?}");
    assert!(*open[0] - pushed < Duration::from_secs(1), "{reads:?}");
    for pair in open.windows(2) {
        let retry = *pair[1] - *pair[0];
        assert!(
            retry > Duration::from_millis(1500) && retry < Duration::from_secs(3),
            "{retry:?}"
        );
    }
    let after: Vec<_> = reads.iter().filter(|read| **read >= closed).collect();
    assert!(*after[0] - closed < Duration::from_secs(1), "{reads:?}");
    let gap = *after[1] - *after[0];
    assert!(
        gap > Duration::from_millis(1500) && gap < Duration::from_secs(3),
        "{gap:?}"
    );
}
