//! Explicitly started, read-only roster polling, independent of tmux discovery.

use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use tmux_workspaces_roster::{Snapshot, views};

mod location;
pub use location::provider_directory;

/// One already-enabled provider, with a private cache on its own I/O worker.
///
/// Updates coalesce into one slot. Failures retain metadata and mark it stale;
/// the separate session-discovery worker remains the source of availability.
/// Merely importing this module reads no provider files or environment values.
pub struct RosterSource {
    updates: Arc<Mutex<Option<Snapshot>>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl RosterSource {
    /// Read immediately, then two seconds after each completed attempt.
    ///
    /// The caller first decides whether the provider is enabled and resolves
    /// its directory. Configuration is re-read on every attempt, including after
    /// errors. `wake` runs on this worker after a complete snapshot is published.
    /// No product startup, terminal attachment or session mutation is enabled.
    pub fn start(
        data_dir: PathBuf,
        url: Option<String>,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let pending = Arc::clone(&updates);
        let (stop, stopped) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("roster polling".into())
            .spawn(move || {
                let mut snapshot = Snapshot::default();
                let mut unavailable = None;
                loop {
                    let started = std::time::Instant::now();
                    snapshot.fetch_configured(&data_dir, url.as_deref());
                    let failed = snapshot.error.is_some();
                    if unavailable != Some(failed) {
                        tracing::debug!(
                            event = "roster_availability_changed",
                            available = !failed,
                            count = snapshot.states.len(),
                            "Agent roster availability changed"
                        );
                        unavailable = Some(failed);
                    }
                    tracing::trace!(
                        event = "roster_refresh_finished",
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        success = !failed,
                        "Agent roster refreshed"
                    );
                    *pending.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some(snapshot.clone());
                    wake();
                    if !matches!(
                        stopped.recv_timeout(Duration::from_secs(2)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
            })?;
        Ok(Self {
            updates,
            stop,
            worker: Some(worker),
        })
    }

    /// Take the newest complete snapshot, including failures replacing unread
    /// successes. Keep its stale flag when combining it with session discovery.
    pub fn take_update(&self) -> Option<Snapshot> {
        self.updates
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

impl Drop for RosterSource {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            // Finish the current configuration read/request, interrupt the
            // polling wait, and leave no worker running after this handle.
            let _ = worker.join();
        }
    }
}

/// The newest data for one Backbone view, or why there is none, with the
/// request it answers.
pub type ViewUpdate = (views::Request, Result<views::Data, &'static str>);

/// Polls the data of one view while its pane is shown, on its own I/O worker.
///
/// The request can change in place, such as a new usage range, and is read
/// again at once. Dropping the handle stops polling. Like the roster, every
/// read re-resolves configuration and nothing is written to Backbone.
pub struct ViewSource {
    /// Cleared when the view is gone, so a library read stops early.
    live: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    request: Arc<Mutex<views::Request>>,
    generation: Arc<AtomicU64>,
    updates: Arc<Mutex<Option<ViewUpdate>>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl ViewSource {
    /// Read immediately, then `every` after each completed read.
    pub fn start(
        data_dir: PathBuf,
        request: views::Request,
        every: Duration,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let request = Arc::new(Mutex::new(request));
        let updates = Arc::new(Mutex::new(None));
        let (shared, pending) = (Arc::clone(&request), Arc::clone(&updates));
        let (stop, stopped) = mpsc::channel();
        let live = Arc::new(AtomicBool::new(true));
        let wanted = Arc::clone(&live);
        let active = Arc::new(AtomicBool::new(true));
        let visible = Arc::clone(&active);
        let generation = Arc::new(AtomicU64::new(0));
        let current_generation = Arc::clone(&generation);
        let worker = thread::Builder::new()
            .name("view polling".into())
            .spawn(move || {
                loop {
                    // A burst of selection changes needs one read of the
                    // latest selection, not one read per queued wakeup.
                    while stopped.try_recv().is_ok() {}
                    if !wanted.load(Ordering::Relaxed) {
                        break;
                    }
                    if !visible.load(Ordering::Relaxed) {
                        if stopped.recv().is_err() {
                            break;
                        }
                        continue;
                    }
                    let (request, generation) = {
                        let request = shared.lock().unwrap_or_else(|e| e.into_inner());
                        (request.clone(), current_generation.load(Ordering::Relaxed))
                    };
                    let started = std::time::Instant::now();
                    let update = views::read(&data_dir, None, &request, &|| {
                        wanted.load(Ordering::Relaxed)
                            && visible.load(Ordering::Relaxed)
                            && current_generation.load(Ordering::Relaxed) == generation
                    });
                    tracing::trace!(
                        event = "view_refresh_finished",
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        success = update.is_ok(),
                        "Information view refreshed"
                    );
                    if let Err(reason) = &update {
                        tracing::debug!(
                            event = "view_refresh_failed",
                            reason,
                            "Information view refresh failed"
                        );
                    }
                    let explicit = matches!(
                        request,
                        views::Request::Document(_)
                            | views::Request::Reports(views::ReportQuery::Documents(..), _)
                    );
                    let interval = match request {
                        views::Request::Agent(_, views::Part::Usage(_)) => Duration::from_secs(15),
                        _ => every,
                    };
                    let published = {
                        let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
                        if wanted.load(Ordering::Relaxed)
                            && visible.load(Ordering::Relaxed)
                            && current_generation.load(Ordering::Relaxed) == generation
                        {
                            *pending = Some((request, update));
                            true
                        } else {
                            false
                        }
                    };
                    if published {
                        wake();
                    }
                    if explicit {
                        if stopped.recv().is_err() {
                            break;
                        }
                        continue;
                    }
                    match stopped.recv_timeout(interval) {
                        Err(mpsc::RecvTimeoutError::Timeout) | Ok(()) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self {
            live,
            active,
            request,
            generation,
            updates,
            stop,
            worker: Some(worker),
        })
    }

    /// Replace the request and read it now. An update for the old request
    /// may still arrive; compare its request before using it.
    pub fn set_request(&self, request: views::Request) {
        {
            let mut current = self.request.lock().unwrap_or_else(|e| e.into_inner());
            *current = request;
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.stop.send(());
    }

    /// Pause reads while another pane is expanded, preserving this view's
    /// selection and data. Showing it again requests an immediate refresh.
    pub fn set_active(&self, active: bool) {
        let mut pending = self.updates.lock().unwrap_or_else(|e| e.into_inner());
        if self.active.swap(active, Ordering::Relaxed) != active {
            self.generation.fetch_add(1, Ordering::Relaxed);
            *pending = None;
            let _ = self.stop.send(());
        }
    }

    /// Take the newest read, if one arrived since the last call.
    pub fn take_update(&self) -> Option<ViewUpdate> {
        self.updates
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

impl Drop for ViewSource {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Relaxed);
        let (stop, _) = mpsc::channel();
        // Dropping the only sender disconnects the worker's wait.
        drop(std::mem::replace(&mut self.stop, stop));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, fs,
        io::{Read, Write},
        net::TcpListener,
        process::Command,
        time::Instant,
    };

    #[test]
    fn view_requests_cancel_old_chains_and_coalesce_selection_changes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "roster::tests::view_poll_fixture_child",
                "--nocapture",
            ])
            .env("VIEW_POLL_FIXTURE_DIR", directory.path())
            .env_remove("BACKBONE_SOCKET_PATH")
            .env("BACKBONE_API_KEY", "")
            .env(
                "BACKBONE_PORT",
                listener.local_addr().unwrap().port().to_string(),
            )
            .spawn()
            .unwrap();
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut paths = Vec::new();
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            let (mut socket, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            paths.push(
                String::from_utf8(request)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            );
            if paths.len() == 1 {
                fs::write(directory.path().join("reading"), "").unwrap();
                while !directory.path().join("changed").exists() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{\"items\":[]}").unwrap();
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
        }
        assert!(child.wait().unwrap().success());
        assert_eq!(paths.len(), 5, "{paths:?}");
        assert!(paths[0].starts_with("GET /api/agents "));
        assert!(paths[1].starts_with("GET /api/usage?"));
        assert!(paths[2].starts_with("GET /api/usage?"));
        assert!(paths[3].starts_with("GET /api/documents/7 "));
        assert!(paths[4].starts_with("GET /api/documents/7 "));
    }

    #[test]
    fn view_poll_fixture_child() {
        let Some(directory) = env::var_os("VIEW_POLL_FIXTURE_DIR").map(PathBuf::from) else {
            return;
        };
        let (wake, woken) = mpsc::channel();
        let source = ViewSource::start(
            directory.clone(),
            views::Request::Team,
            Duration::from_secs(60),
            move || {
                let _ = wake.send(());
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !directory.join("reading").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..20 {
            source.set_request(views::Request::Usage(views::Range::Hour));
            source.set_request(views::Request::Usage(views::Range::Week));
        }
        fs::write(directory.join("changed"), "").unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        {
            let pending = source.updates.lock().unwrap();
            let (request, update) = pending.as_ref().unwrap();
            assert_eq!(*request, views::Request::Usage(views::Range::Week));
            assert!(update.is_ok(), "{update:?}");
        }
        assert!(woken.recv_timeout(Duration::from_millis(200)).is_err());
        source.set_active(false);
        assert!(source.take_update().is_none());
        source.set_request(views::Request::Usage(views::Range::Hour));
        assert!(woken.recv_timeout(Duration::from_millis(200)).is_err());
        source.set_active(true);
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            source.take_update().unwrap().0,
            views::Request::Usage(views::Range::Hour)
        );
        drop(source);

        // Document reads, including failed reads, wait for an explicit refresh.
        let (wake, woken) = mpsc::channel();
        let source = ViewSource::start(
            directory,
            views::Request::Document(7),
            Duration::from_millis(30),
            move || {
                let _ = wake.send(());
            },
        )
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(source.take_update().unwrap().1.is_err());
        assert!(woken.recv_timeout(Duration::from_millis(200)).is_err());
        source.set_request(views::Request::Document(7));
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(source);
    }

    #[test]
    fn polling_rereads_configuration_coalesces_failures_and_recovers() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let socket_directory = tempfile::tempdir().unwrap();
        fs::set_permissions(socket_directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = socket_directory.path().join("api.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for body in [
                r#"{"items":[{"name":"worker","state":"busy"}]}"#,
                "not JSON",
                r#"{"items":[]}"#,
            ] {
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "fixture received no request");
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                requests.push(String::from_utf8(request).unwrap());
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".env"), "BACKBONE_API_KEY=poll-key-1").unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "roster::tests::polling_fixture_child",
                "--nocapture",
            ])
            .env("ROSTER_POLL_FIXTURE_DIR", directory.path())
            .env("BACKBONE_API_KEY", "")
            .env("BACKBONE_SOCKET_PATH", &socket_path)
            .status()
            .unwrap();
        assert!(status.success());
        for (index, request) in server.join().unwrap().iter().enumerate() {
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("get /api/agents http/1.1\r\n"));
            assert!(request.contains(&format!("authorization: bearer poll-key-{}\r\n", index + 1)));
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn polling_fixture_child() {
        let Some(directory) = env::var_os("ROSTER_POLL_FIXTURE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let (wake, woken) = mpsc::channel();
        let source = RosterSource::start(directory.clone(), None, move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        // Leave success unread so the following failure must replace it.
        let first = source.updates.lock().unwrap().as_ref().unwrap().clone();
        assert!(first.error.is_none());
        assert_eq!(first.states["worker"], "busy");
        fs::write(directory.join(".env"), "BACKBONE_API_KEY=poll-key-2").unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let failed = source.take_update().unwrap();
        assert!(failed.error.is_some());
        assert_eq!(failed.states, first.states);
        assert_eq!(failed.observed_at, first.observed_at);
        fs::write(directory.join(".env"), "BACKBONE_API_KEY=poll-key-3").unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let recovered = source.take_update().unwrap();
        assert!(recovered.error.is_none() && recovered.states.is_empty());
        assert!(recovered.observed_at > first.observed_at);
        drop(source);
        assert_eq!(
            woken.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
