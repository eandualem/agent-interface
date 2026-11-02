//! Roster polling owned by one verified profile connection.

use super::*;
use agent_interface_roster::profiles::{Connection, Error, Identity};

/// One complete observation, including the origin of retained stale metadata.
#[derive(Clone, Debug)]
pub struct BoundRosterUpdate {
    /// The verified instance that owns this worker and its cache.
    pub identity: Identity,
    /// Last successful roster, marked stale after a failed read.
    pub snapshot: Snapshot,
    /// The latest failure category, without provider prose or credentials.
    pub error: Option<Error>,
}

/// An independent cache and polling worker for one immutable connection.
/// There is no operation to rebind a running worker to another profile.
pub struct BoundRosterSource {
    updates: Arc<Mutex<Option<BoundRosterUpdate>>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl BoundRosterSource {
    /// Read immediately, then two seconds after each completed attempt.
    /// The connection retains its verified tuple, endpoint and credential for
    /// this worker's lifetime. `wake` runs after publishing a complete update.
    pub fn start(connection: Connection, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let pending = Arc::clone(&updates);
        let (stop, stopped) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("bound roster polling".into())
            .spawn(move || {
                let mut update = BoundRosterUpdate {
                    identity: connection.identity().clone(),
                    snapshot: Snapshot::default(),
                    error: None,
                };
                loop {
                    match connection.roster() {
                        Ok(snapshot) => {
                            update.snapshot = snapshot;
                            update.error = None;
                        }
                        Err(error) => {
                            update.snapshot.unavailable();
                            update.error = Some(error);
                        }
                    }
                    *pending.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some(update.clone());
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

    /// Take the newest complete update. Failures replace unread successes and
    /// preserve only this worker's last successful observation and timestamp.
    pub fn take_update(&self) -> Option<BoundRosterUpdate> {
        self.updates
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

impl Drop for BoundRosterSource {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            // The in-flight bounded request finishes before this handle drops.
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
