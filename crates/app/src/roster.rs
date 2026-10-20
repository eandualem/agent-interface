//! Explicitly started, read-only roster polling, independent of tmux discovery.

use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use tmux_workspaces_roster::Snapshot;

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
    /// Read immediately, then five seconds after each completed attempt.
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
                loop {
                    snapshot.fetch_configured(&data_dir, url.as_deref());
                    *pending.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some(snapshot.clone());
                    wake();
                    if !matches!(
                        stopped.recv_timeout(Duration::from_secs(5)),
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
    fn polling_rereads_configuration_coalesces_failures_and_recovers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
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
            .env("BACKBONE_PORT", port)
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
