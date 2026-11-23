//! Roster polling owned by one verified profile connection.

use std::{
    io,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Between reads without the push stream.
const POLL: Duration = Duration::from_secs(2);
/// From the start of one read to the next while the push stream is open and
/// the last roster and runs reads succeeded: the service's heartbeat, and half the 30 seconds
/// after which a roster or runs list is no longer current. The stream carries
/// states, waits and runs, but not an agent leaving, a new issue without a new
/// state, or swarms and machines.
const STREAMED: Duration = Duration::from_secs(15);

use agent_interface_roster::Snapshot;
use agent_interface_roster::profiles::{
    Change, Connection, Error, Identity,
    runs::{Book, Runs},
};

/// One complete observation, including the origin of retained stale metadata.
#[derive(Clone, Debug)]
pub struct BoundRosterUpdate {
    /// The verified instance that owns this worker and its cache.
    pub identity: Identity,
    /// Last successful roster, marked stale after a failed read.
    pub snapshot: Snapshot,
    /// The latest failure category, without provider prose or credentials.
    pub error: Option<Error>,
    /// Background runs as last read; kept as they were when a read fails.
    pub runs: Runs,
}

/// An independent cache and polling worker for one immutable connection.
/// There is no operation to rebind a running worker to another profile.
pub struct BoundRosterSource {
    updates: Arc<Mutex<Option<BoundRosterUpdate>>>,
    /// True stops the polling worker; false asks it to read now.
    stop: mpsc::Sender<bool>,
    /// Cleared before a stop, so the worker starts no further runs read.
    polling: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// Follows the Rust service's push stream, when the connection has one.
    stream: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

impl BoundRosterSource {
    /// Read immediately, then two seconds after each completed attempt, or 15
    /// seconds after the start of a good read while the Rust service's push
    /// stream is open. The stream, when the
    /// connection has one, changes an agent's state or wait at once, and asks
    /// for a read at once for anything else and when it ends. Runs are read after each roster is published, so states
    /// never wait on them. The connection retains its verified identity and
    /// socket for this worker's lifetime. `wake` runs after publishing a
    /// complete update.
    pub fn start(connection: Connection, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let shown = Arc::new(Mutex::new(Shown {
            update: BoundRosterUpdate {
                identity: connection.identity().clone(),
                snapshot: Snapshot::default(),
                error: None,
                runs: Runs::default(),
            },
            applied: None,
            pending: Arc::clone(&updates),
            wake: Box::new(wake),
        }));
        let (stop, stopped) = mpsc::channel();
        let (watched, refresh, streamed) = (connection.clone(), stop.clone(), Arc::clone(&shown));
        let polling = Arc::new(AtomicBool::new(true));
        let wanted = Arc::clone(&polling);
        let streaming = Arc::new(AtomicBool::new(false));
        let open = Arc::clone(&streaming);
        let worker = thread::Builder::new()
            .name("bound roster polling".into())
            .spawn(move || {
                let mut book = Book::default();
                let live = || wanted.load(Ordering::Relaxed);
                // Publish the runs only when the book changed.
                let show_runs = |runs: &Runs| {
                    let mut shown = lock(&shown);
                    if *runs != shown.update.runs {
                        shown.update.runs = runs.clone();
                        shown.publish();
                    }
                };
                loop {
                    let started = std::time::Instant::now();
                    lock(&shown).applied = Some(Vec::new());
                    let read = connection.roster();
                    let listed = {
                        let mut shown = lock(&shown);
                        let applied = shown.applied.take().unwrap_or_default();
                        match read {
                            Ok(mut snapshot) => {
                                // The stream's changes during the read are at
                                // least as new as what it read.
                                for change in &applied {
                                    snapshot.apply(change);
                                }
                                shown.update.snapshot = snapshot;
                                shown.update.error = None;
                            }
                            Err(error) => {
                                shown.update.snapshot.unavailable();
                                shown.update.error = Some(standing(shown.update.error, error));
                            }
                        }
                        // The pass's first publication already says whether the
                        // runs are too old to show, whatever the roster read did.
                        book.expire();
                        if *book.runs() != shown.update.runs {
                            shown.update.runs = book.runs().clone();
                        }
                        shown.publish();
                        shown
                            .update
                            .error
                            .is_none()
                            .then(|| shown.update.snapshot.clone())
                    };
                    let mut good = listed.is_some();
                    if let Some(snapshot) = listed
                        && live()
                    {
                        // The new list shows before its ended runs are read.
                        let gone = book.list(&connection, &snapshot);
                        good = !gone.missed();
                        show_runs(book.runs());
                        book.settle(&connection, gone, &live);
                        show_runs(book.runs());
                    }
                    // A failed read is tried again soon: the stream's states
                    // wait for a good roster, and runs go stale 30 seconds
                    // after the last good list.
                    let wait = if good && streaming.load(Ordering::Relaxed) {
                        STREAMED.saturating_sub(started.elapsed()).max(POLL)
                    } else {
                        POLL
                    };
                    match stopped.recv_timeout(wait) {
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
            polling,
            worker: Some(worker),
            stream: None,
        };
        // Without a stream worker the roster is still polled.
        source.stream = watch(watched, refresh, streamed, open)
            .map(Some)
            .unwrap_or_else(|error| {
                tracing::warn!(event = "roster_stream_not_started", %error, "Agent state stream not followed");
                None
            });
        Ok(source)
    }

    /// Take the newest complete update. Failures replace unread successes and
    /// preserve only this worker's last successful observation and timestamp.
    pub fn take_update(&self) -> Option<BoundRosterUpdate> {
        lock(&self.updates).take()
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

/// Follow the connection's push stream on a worker of its own, applying each
/// agent's state and wait to `shown` and asking the polling worker for a read
/// for any other change, or one `shown` can't take. `open` is true from the
/// stream's first frame until it ends, when a read is asked for at once. The
/// stream is opened again two seconds after it breaks, and not again once the
/// service says it has none.
fn watch(
    connection: Connection,
    refresh: mpsc::Sender<bool>,
    shown: Arc<Mutex<Shown>>,
    open: Arc<AtomicBool>,
) -> io::Result<(Arc<AtomicBool>, JoinHandle<()>)> {
    let live = Arc::new(AtomicBool::new(true));
    let wanted = Arc::clone(&live);
    let worker = thread::Builder::new()
        .name("bound roster stream".into())
        .spawn(move || {
            let live = || wanted.load(Ordering::Relaxed);
            while live() {
                let ended = connection.watch(&live, &mut |change| {
                    open.store(true, Ordering::Relaxed);
                    let mut shown = lock(&shown);
                    // A failed read's stale roster waits for a good one.
                    if shown.update.error.is_none() && shown.update.snapshot.apply(&change) {
                        // An older service doesn't push the issue an agent
                        // works on, so its new state shows at once and is
                        // read again behind it.
                        let read = matches!(change, Change::State { issue: None, .. });
                        if let Some(applied) = &mut shown.applied {
                            applied.push(change);
                        }
                        shown.publish();
                        if read {
                            let _ = refresh.send(false);
                        }
                    } else {
                        let _ = refresh.send(false);
                    }
                });
                // What the stream would have pushed while it is down is read.
                if open.swap(false, Ordering::Relaxed) {
                    let _ = refresh.send(false);
                }
                if ended == Err(Error::Unsupported) {
                    break;
                }
                if live() {
                    tracing::debug!(event = "roster_stream_ended", reason = ?ended, "Agent state stream ended");
                    thread::park_timeout(Duration::from_secs(2));
                }
            }
        })?;
    Ok((live, worker))
}

/// The update both workers change, under one lock, so each publication is
/// whole.
struct Shown {
    update: BoundRosterUpdate,
    /// The stream's changes since the roster read that is out began; none
    /// between reads.
    applied: Option<Vec<Change>>,
    pending: Arc<Mutex<Option<BoundRosterUpdate>>>,
    wake: Box<dyn Fn() + Send>,
}

impl Shown {
    fn publish(&self) {
        *lock(&self.pending) = Some(self.update.clone());
        (self.wake)();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for BoundRosterSource {
    fn drop(&mut self) {
        self.polling.store(false, Ordering::Relaxed);
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
