//! Read-only source-session polling, independent of the product interface.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agent_interface_roster::profiles::{Connection, Error as ProfileError, Identity};
use agent_interface_tmux::discovery::read_sessions;

pub use crate::library::location::source_socket;

/// A complete replacement for the source's previous availability.
#[derive(Debug)]
pub struct Observation {
    /// Captured profile origin, absent only for explicit legacy socket discovery.
    pub identity: Option<Identity>,
    /// Typed profile verification failure; failed observations contain no sessions.
    pub profile_error: Option<ProfileError>,
    /// Exact names from the latest successful query; empty on failure.
    pub sessions: Vec<String>,
    /// None for a successful query, including an absent server.
    pub error: Option<String>,
    /// When this successful query completed; none on failure.
    pub observed_at: Option<Instant>,
}

enum Source {
    Legacy(PathBuf),
    Bound(Connection),
}
impl Source {
    fn observe(&self, live: &dyn Fn() -> bool) -> Option<Observation> {
        if !live() {
            return None;
        }
        let identity = match self {
            Self::Legacy(_) => None,
            Self::Bound(connection) => Some(connection.identity().clone()),
        };
        let refused = |error| Observation {
            identity: identity.clone(),
            profile_error: Some(error),
            sessions: Vec::new(),
            error: Some("The original profile could not verify its session source.".into()),
            observed_at: None,
        };
        let socket = match self {
            Self::Legacy(socket) => socket.as_path(),
            Self::Bound(connection) => {
                if let Err(error) = connection.verify() {
                    return live().then(|| refused(error));
                }
                connection.tmux_socket()
            }
        };
        if !live() {
            return None;
        }
        let result = read_sessions(socket);
        if !live() {
            return None;
        }
        if let Self::Bound(connection) = self {
            // Reject data if the provider changed while the tmux query ran.
            if let Err(error) = connection.verify() {
                return live().then(|| refused(error));
            }
        }
        if !live() {
            return None;
        }
        Some(match result {
            Ok(sessions) => Observation {
                identity,
                profile_error: None,
                sessions,
                error: None,
                observed_at: Some(Instant::now()),
            },
            Err(error) => Observation {
                identity,
                profile_error: None,
                sessions: Vec::new(),
                error: Some(error.to_string()),
                observed_at: None,
            },
        })
    }
}

/// One explicit source socket polled on a dedicated worker.
///
/// Only discovery establishes availability. Saved references or future metadata
/// overlays must not turn a failed query into an online claim. Updates coalesce
/// into one slot, including failures replacing unread successes.
pub struct SessionSource {
    state: Arc<Mutex<PollState>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

struct PollState {
    active: bool,
    generation: u64,
    update: Option<Observation>,
}

impl SessionSource {
    /// Query immediately on the worker, then every five seconds. The caller
    /// resolves `socket` first. No default-server lookup or attachment occurs.
    /// `wake` runs on the worker after each complete observation is published.
    pub fn start(socket: PathBuf, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        Self::start_source(Source::Legacy(socket), wake)
    }

    /// Poll only the captured profile's authenticated tmux socket. Verify the
    /// full binding before and after each query; never resolve a fallback source.
    /// This is read-only and never starts a server or attaches a client.
    pub fn start_bound(
        connection: Connection,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        Self::start_source(Source::Bound(connection), wake)
    }

    fn start_source(source: Source, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(PollState {
            active: true,
            generation: 0,
            update: None,
        }));
        let pending = Arc::clone(&state);
        let (stop, stopped) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("session discovery".into())
            .spawn(move || {
                loop {
                    loop {
                        match stopped.try_recv() {
                            Ok(()) => {}
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => return,
                        }
                    }
                    let (active, generation) = {
                        let state = pending.lock().unwrap_or_else(|e| e.into_inner());
                        (state.active, state.generation)
                    };
                    if !active {
                        if stopped.recv().is_err() {
                            break;
                        }
                        continue;
                    }
                    let live = || {
                        let state = pending.lock().unwrap_or_else(|error| error.into_inner());
                        state.active && state.generation == generation
                    };
                    let Some(observation) = source.observe(&live) else {
                        continue;
                    };
                    let published = {
                        let mut state = pending.lock().unwrap_or_else(|e| e.into_inner());
                        if state.active && state.generation == generation {
                            state.update = Some(observation);
                            true
                        } else {
                            false
                        }
                    };
                    if published {
                        wake();
                    }
                    match stopped.recv_timeout(Duration::from_secs(5)) {
                        Err(mpsc::RecvTimeoutError::Timeout) | Ok(()) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self {
            state,
            stop,
            worker: Some(worker),
        })
    }

    /// Pause discovery while hidden. Discard pending observations and refresh
    /// immediately on reveal; a read begun before the change cannot publish.
    pub fn set_active(&self, active: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.active != active {
            state.active = active;
            state.generation = state.generation.wrapping_add(1);
            state.update = None;
            let _ = self.stop.send(());
        }
    }

    /// Take the newest complete observation. Replace previous availability even
    /// when it carries an error; old names are not evidence of live sessions.
    pub fn take_update(&self) -> Option<Observation> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update
            .take()
    }
}

impl Drop for SessionSource {
    fn drop(&mut self) {
        self.set_active(false);
        let (stop, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.stop, stop));
        if let Some(worker) = self.worker.take() {
            // A query is bounded; bound mode also waits for its current identity probe. No child or
            // polling thread outlives this handle after the join completes.
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_test_support::TmuxServer;
    use std::os::unix::fs::symlink;

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
            sessions
                .state
                .lock()
                .unwrap()
                .update
                .as_ref()
                .unwrap()
                .sessions,
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

    #[test]
    fn hidden_discovery_discards_pending_names_and_refreshes_on_reveal() {
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let source = SessionSource::start(socket.trim_end().into(), move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        source.set_active(false);
        assert!(source.take_update().is_none());
        server
            .run(&["rename-session", "-t", TmuxServer::SESSION, "renamed"])
            .unwrap();
        assert!(woken.recv_timeout(Duration::from_millis(5200)).is_err());
        source.set_active(true);
        // This is shorter than the normal five-second polling interval.
        woken.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(source.take_update().unwrap().sessions, ["renamed"]);
        drop(source);
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
    }
}

#[cfg(test)]
#[path = "sessions/bound_tests.rs"]
mod bound_tests;
