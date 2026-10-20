//! Read-only library polling on the app's storage worker.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tmux_workspaces_store::{Document, Error, Store};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The latest library change. A failed read retains any valid document still
/// waiting for the caller; a caller also keeps its previously displayed document.
#[derive(Debug, Default)]
pub struct Update {
    /// A new validated document, or none when only the error changed.
    pub document: Option<Document>,
    /// The current read error. None clears an earlier error.
    pub error: Option<Error>,
}

/// One read-only SQLite connection owned by a background thread.
///
/// Updates coalesce into one slot. Dropping the handle stops polling and joins
/// the worker; it never changes the database or stops any shell.
pub struct Library {
    updates: Arc<Mutex<Option<Update>>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Library {
    /// Start reading an already selected library directory without blocking on
    /// SQLite in the calling thread. `wake` runs after an update is published.
    ///
    /// An initial open failure is published and ends the worker. Once opened,
    /// read failures are retried, with unchanged errors reported only once.
    pub fn start(directory: PathBuf, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let (stop, stopped) = mpsc::channel();
        let pending = Arc::clone(&updates);
        let worker = thread::Builder::new()
            .name("storage worker".into())
            .spawn(move || {
                let store = match Store::open(&directory) {
                    Ok(store) => store,
                    Err(error) => {
                        publish(&pending, Err(error), &wake);
                        return;
                    }
                };
                let mut version = None;
                let mut last_error = None;
                loop {
                    let result = store.data_version().and_then(|current| {
                        if version == Some(current) && last_error.is_none() {
                            return Ok(None);
                        }
                        let document = store.load()?;
                        // The token was read before load. A racing commit still
                        // changes the next poll, even if this read already saw it.
                        version = Some(current);
                        Ok(Some(document))
                    });
                    match result {
                        Ok(Some(document)) => {
                            last_error = None;
                            publish(&pending, Ok(document), &wake);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let description = error.to_string();
                            if last_error.as_ref() != Some(&description) {
                                last_error = Some(description);
                                publish(&pending, Err(error), &wake);
                            }
                        }
                    }
                    if !matches!(
                        stopped.recv_timeout(POLL_INTERVAL),
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

    /// Take the latest update, if any. Reconcile the replacement model with
    /// `Layout::retain_navigation` before displaying it; when it returns true,
    /// move input focus to navigation. This worker does not own input focus.
    pub fn take_update(&self) -> Option<Update> {
        self.updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn publish(updates: &Mutex<Option<Update>>, result: Result<Document, Error>, wake: &impl Fn()) {
    {
        let mut pending = updates.lock().unwrap_or_else(|e| e.into_inner());
        let update = pending.get_or_insert_with(Update::default);
        match result {
            Ok(document) => {
                update.document = Some(document);
                update.error = None;
            }
            Err(error) => update.error = Some(error),
        }
    }
    wake();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn observes_peer_writes_preserves_valid_pending_data_and_reports_open_failure() {
        let directory = tempfile::tempdir().unwrap();
        let writer = Connection::open(directory.path().join("layouts.db")).unwrap();
        writer
            .execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            .unwrap();
        let valid = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Initial","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        writer
            .execute("INSERT INTO terminal_layout VALUES (1, ?1)", [valid])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let library = Library::start(directory.path().into(), move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        // Leave the valid document pending while a subsequent read fails.
        writer
            .execute("UPDATE terminal_layout SET value = '{}'", [])
            .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert_eq!(
            update.document.unwrap().model().workspaces[0].name,
            "Initial"
        );
        assert!(update.error.is_some());
        assert!(matches!(
            woken.recv_timeout(Duration::from_millis(600)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        writer
            .execute(
                "UPDATE terminal_layout SET value = ?1",
                [valid.replace("Initial", "Changed")],
            )
            .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert_eq!(
            update.document.unwrap().model().workspaces[0].name,
            "Changed"
        );
        assert!(update.error.is_none());
        drop(library);

        let missing = directory.path().join("missing");
        let (wake, woken) = mpsc::channel();
        let library = Library::start(missing.clone(), move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert!(update.document.is_none());
        assert!(update.error.is_some());
        drop(library);
        assert!(
            !missing.exists(),
            "the worker must not initialize a library"
        );
    }
}
