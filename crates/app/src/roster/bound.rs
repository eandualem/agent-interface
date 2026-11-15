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
    /// True stops the polling worker; false asks it to read now.
    stop: mpsc::Sender<bool>,
    worker: Option<JoinHandle<()>>,
    /// Follows the Rust service's push stream, when the connection has one.
    stream: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

impl BoundRosterSource {
    /// Read immediately, then two seconds after each completed attempt, and
    /// at once when the Rust service's push stream reports a changed state.
    /// The connection retains its verified tuple, endpoint and credential for
    /// this worker's lifetime. `wake` runs after publishing a complete update.
    pub fn start(connection: Connection, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let pending = Arc::clone(&updates);
        let (stop, stopped) = mpsc::channel();
        let (watched, refresh) = (connection.clone(), stop.clone());
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
                            update.error = Some(standing(update.error, error));
                        }
                    }
                    *pending.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some(update.clone());
                    wake();
                    match stopped.recv_timeout(Duration::from_secs(2)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        // Several changes at once need one read; a stop among
                        // them still stops.
                        Ok(false) => {
                            if stopped.try_iter().any(|stop| stop) {
                                break;
                            }
                        }
                        Ok(true) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        let mut source = Self {
            updates,
            stop,
            worker: Some(worker),
            stream: None,
        };
        // Without a stream worker the roster is still polled.
        source.stream = watch(watched, refresh).unwrap_or_else(|error| {
            tracing::warn!(event = "roster_stream_not_started", %error, "Agent state stream not followed");
            None
        });
        Ok(source)
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

/// The failure to report after `error`. A refusal stands until a read
/// succeeds: a later read that could not be made does not replace it.
fn standing(reported: Option<Error>, error: Error) -> Error {
    match reported {
        Some(refusal) if refusal != Error::Unavailable && error == Error::Unavailable => refusal,
        _ => error,
    }
}

/// Follow the connection's push stream on a worker of its own, asking the
/// polling worker for a read at each change. The stream is opened again two
/// seconds after it breaks. A connection without one gets no worker.
fn watch(
    connection: Connection,
    refresh: mpsc::Sender<bool>,
) -> io::Result<Option<(Arc<AtomicBool>, JoinHandle<()>)>> {
    if !connection.served() {
        return Ok(None);
    }
    let live = Arc::new(AtomicBool::new(true));
    let wanted = Arc::clone(&live);
    let worker = thread::Builder::new()
        .name("bound roster stream".into())
        .spawn(move || {
            let live = || wanted.load(Ordering::Relaxed);
            while live() {
                let ended = connection.watch(&live, &mut || {
                    let _ = refresh.send(false);
                });
                if ended == Err(Error::Unsupported) {
                    break;
                }
                if live() {
                    tracing::debug!(event = "roster_stream_ended", reason = ?ended, "Agent state stream ended");
                    thread::park_timeout(Duration::from_secs(2));
                }
            }
        })?;
    Ok(Some((live, worker)))
}

impl Drop for BoundRosterSource {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some((live, worker)) = self.stream.take() {
            live.store(false, Ordering::Relaxed);
            worker.thread().unpark();
            // The stream notices within a quarter of a second.
            let _ = worker.join();
        }
        if let Some(worker) = self.worker.take() {
            // The in-flight bounded request finishes before this handle drops.
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
