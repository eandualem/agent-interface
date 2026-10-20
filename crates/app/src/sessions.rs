//! Read-only source-session polling, independent of the product interface.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tmux_workspaces_tmux::discovery::read_sessions;

pub use crate::library::location::source_socket;

mod metadata;
pub use metadata::{CombinedObservation, Origin, Session, with_roster};

/// A complete replacement for the source's previous availability.
#[derive(Debug)]
pub struct Observation {
    /// Exact names from the latest successful query; empty on failure.
    pub sessions: Vec<String>,
    /// None for a successful query, including an absent server.
    pub error: Option<String>,
    /// When this successful query completed; none on failure.
    pub observed_at: Option<Instant>,
}

/// One explicit source socket polled on a dedicated worker.
///
/// Only discovery establishes availability. Saved references or future metadata
/// overlays must not turn a failed query into an online claim. Updates coalesce
/// into one slot, including failures replacing unread successes.
pub struct SessionSource {
    updates: Arc<Mutex<Option<Observation>>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl SessionSource {
    /// Query immediately on the worker, then every five seconds. The caller
    /// resolves `socket` first. No default-server lookup or attachment occurs.
    /// `wake` runs on the worker after each complete observation is published.
    pub fn start(socket: PathBuf, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let pending = Arc::clone(&updates);
        let (stop, stopped) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("session discovery".into())
            .spawn(move || {
                loop {
                    let observation = match read_sessions(&socket) {
                        Ok(sessions) => Observation {
                            sessions,
                            error: None,
                            observed_at: Some(Instant::now()),
                        },
                        Err(error) => Observation {
                            sessions: Vec::new(),
                            error: Some(error.to_string()),
                            observed_at: None,
                        },
                    };
                    *pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(observation);
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

    /// Take the newest complete observation. Replace previous availability even
    /// when it carries an error; old names are not evidence of live sessions.
    pub fn take_update(&self) -> Option<Observation> {
        self.updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

impl Drop for SessionSource {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            // A running query has its own three-second deadline. No child or
            // polling thread outlives this handle after the join completes.
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tmux_workspaces_test_support::TmuxServer;

    #[test]
    fn failure_replaces_pending_online_names_and_polling_recovers() {
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let source = directory.path().join("source");
        symlink(socket.trim_end(), &source).unwrap();
        let (wake, woken) = mpsc::channel();
        let sessions = SessionSource::start(source.clone(), move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        // Leave the successful observation unread so a later failure must
        // replace it, rather than preserve an old claim of availability.
        assert_eq!(
            sessions.updates.lock().unwrap().as_ref().unwrap().sessions,
            [TmuxServer::SESSION]
        );
        std::fs::remove_file(&source).unwrap();
        // A loop produces a filesystem error on both Linux and macOS. A
        // regular file produces ECONNREFUSED on Linux (an absent server).
        symlink("source", &source).unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let failed = sessions.take_update().unwrap();
        assert!(failed.sessions.is_empty());
        assert_eq!(failed.error.as_deref(), Some("Session list unavailable"));
        assert!(failed.observed_at.is_none());
        std::fs::remove_file(&source).unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let empty = sessions.take_update().unwrap();
        assert!(empty.sessions.is_empty());
        assert!(empty.error.is_none());
        assert!(empty.observed_at.is_some());
        symlink(socket.trim_end(), &source).unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let recovered = sessions.take_update().unwrap();
        assert_eq!(recovered.sessions, [TmuxServer::SESSION]);
        assert!(recovered.error.is_none());
        assert!(recovered.observed_at > empty.observed_at);
        drop(sessions);
        assert!(woken.recv_timeout(Duration::from_secs(1)).is_err());
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
    }
}
