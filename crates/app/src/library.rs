//! First-run initialization and read-only library polling on the storage worker.

use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use agent_interface_store::{Document, Error, Store};

pub(crate) mod location;
mod shells;
mod window;
pub use location::library_directory;
pub(crate) use shells::prepare_shell;
pub use shells::{
    existing_shell_sessions, existing_shell_socket, ordinary_session, reconnect_shells_from,
    remember_shell_directory, shell_socket_for_creation,
};
pub use window::WindowLayout;

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
    /// Open or create a library without blocking on path lookup or SQLite in the
    /// calling thread. None uses the shared environment/default path precedence.
    /// An absent database starts with an empty workspace; existing files are
    /// never initialized or repaired. `wake` runs after an update is published.
    ///
    /// An initial open failure is published and ends the worker. Once opened,
    /// read failures are retried, with unchanged errors reported only once.
    pub fn start(directory: Option<PathBuf>, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let updates = Arc::new(Mutex::new(None));
        let (stop, stopped) = mpsc::channel();
        let pending = Arc::clone(&updates);
        let worker = thread::Builder::new()
            .name("storage worker".into())
            .spawn(move || {
                let (store, directory) = match location::library_location(directory.as_deref())
                    .and_then(|path| Ok((Store::open_or_create(&path)?, path)))
                {
                    Ok(result) => result,
                    Err(error) => {
                        tracing::warn!(
                            event = "library_open_failed",
                            "Library could not be opened"
                        );
                        publish(&pending, Err(error), &wake);
                        return;
                    }
                };
                tracing::debug!(event = "library_opened", "Library opened");
                let mut version = None;
                let mut last_error = None;
                // The token at which the committed record was last refused.
                // Only another commit can change that result.
                let mut unreadable = None;
                loop {
                    let mut refused = None;
                    let result = store.data_version().and_then(|current| {
                        if (version == Some(current) && last_error.is_none())
                            || unreadable == Some(current)
                        {
                            return Ok(None);
                        }
                        #[cfg(test)]
                        tests::count_load(&directory);
                        let document = store
                            .read_document()?
                            .inspect_err(|_| refused = Some(current))?;
                        if version.is_none()
                            && let Err(error) = remember_once(&directory, shells::remember_server)
                        {
                            tracing::warn!(%error, "Could not record the library's shell server");
                        }
                        // The token was read before load. A racing commit still
                        // changes the next poll, even if this read already saw it.
                        version = Some(current);
                        Ok(Some(document))
                    });
                    // A failed read, such as a lock that ends without a commit,
                    // reads the record again at the next poll.
                    if result.is_err() {
                        unreadable = refused;
                    }
                    match result {
                        Ok(Some(document)) => {
                            tracing::debug!(
                                event = "library_loaded",
                                recovered = last_error.is_some(),
                                "Library document loaded"
                            );
                            last_error = None;
                            publish(&pending, Ok(document), &wake);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let description = error.to_string();
                            if last_error.as_ref() != Some(&description) {
                                tracing::warn!(
                                    event = "library_read_failed",
                                    "Library read failed; last valid document retained"
                                );
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

    /// Take the latest update, if any. Apply replacement documents through
    /// [`WindowLayout::refresh`] to retain this window's navigation and reconcile
    /// local edits. Handle its conflict and active-tree flags before input is
    /// handed off. Read errors leave the window's existing model available.
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

/// The location each library's shell server was last remembered at, by the
/// filesystem identity of its directory and `layouts.db`. A save restarts the
/// reader of the same library, which need not register its server again.
type Identity = (u64, u64, u64, Option<std::time::SystemTime>);
static REMEMBERED: Mutex<BTreeMap<Identity, PathBuf>> = Mutex::new(BTreeMap::new());

/// Run `remember` on the first start of a library in this process, and again
/// once the library at `directory` is another one or has moved. A failure is
/// not recorded, so the next start retries it.
fn remember_once(
    directory: &Path,
    remember: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let identity = std::fs::metadata(directory).and_then(|folder| {
        let database = std::fs::metadata(directory.join("layouts.db"))?;
        // The creation time tells a recreated file from one that reuses its inode.
        Ok((
            folder.dev(),
            folder.ino(),
            database.ino(),
            database.created().ok(),
        ))
    });
    let lock = || REMEMBERED.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(identity) = identity
        && lock()
            .get(&identity)
            .is_some_and(|location| location == directory)
    {
        return Ok(());
    }
    remember(directory)?;
    if let Ok(identity) = identity {
        let mut remembered = lock();
        remembered.retain(|_, location| location != directory);
        remembered.insert(identity, directory.to_owned());
    }
    Ok(())
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
    use std::cell::Cell;

    static LOADS: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

    /// Count record reads per library, so parallel tests do not share a count.
    pub(super) fn count_load(directory: &Path) {
        *LOADS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(directory.to_owned())
            .or_default() += 1;
    }

    fn loads(directory: &Path) -> usize {
        let directory = location::library_location(Some(directory)).unwrap();
        LOADS.lock().unwrap().get(&directory).copied().unwrap_or(0)
    }

    #[test]
    fn a_server_is_remembered_once_until_the_library_is_replaced_or_moved() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        std::fs::create_dir(&first).unwrap();
        std::fs::write(first.join("layouts.db"), "").unwrap();
        let calls = Cell::new(0);
        let remember = |directory: &Path, succeeds: bool| {
            remember_once(directory, |_| {
                calls.set(calls.get() + 1);
                succeeds
                    .then_some(())
                    .ok_or_else(|| io::Error::other("unavailable"))
            })
        };
        assert!(remember(&first, false).is_err());
        remember(&first, true).unwrap();
        // A save restarts the reader of the same library.
        remember(&first, true).unwrap();
        assert_eq!(calls.get(), 2);
        // Another library replaces this one in the same folder.
        std::fs::write(first.join("replacement"), "").unwrap();
        std::fs::rename(first.join("replacement"), first.join("layouts.db")).unwrap();
        remember(&first, true).unwrap();
        assert_eq!(calls.get(), 3);
        // The library moves away and back again.
        let second = root.path().join("second");
        std::fs::rename(&first, &second).unwrap();
        remember(&second, true).unwrap();
        std::fs::rename(&second, &first).unwrap();
        remember(&first, true).unwrap();
        assert_eq!(calls.get(), 5);
    }

    #[test]
    fn an_unreadable_record_is_read_again_only_after_a_commit_or_a_failed_read() {
        let directory = tempfile::tempdir().unwrap();
        let writer = Connection::open(directory.path().join("layouts.db")).unwrap();
        writer
            .execute_batch(
                "CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT);
                 INSERT INTO terminal_layout VALUES (1, '{}')",
            )
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let library = Library::start(Some(directory.path().into()), move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let first = library.take_update().unwrap().error.unwrap().detail;
        // Several polls pass without decoding the unchanged record again.
        std::thread::sleep(POLL_INTERVAL * 3);
        assert_eq!(loads(directory.path()), 1);
        writer
            .execute("UPDATE terminal_layout SET value = 'null'", [])
            .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = library.take_update().unwrap().error.unwrap().detail;
        assert_ne!(first, second);
        std::thread::sleep(POLL_INTERVAL * 3);
        assert_eq!(loads(directory.path()), 2);
        // A lock that ends without a commit leaves the token unchanged, yet the
        // reader reads the record again rather than keep showing the lock.
        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let locked = library.take_update().unwrap().error.unwrap().detail;
        assert_eq!(locked, "database is locked");
        writer.execute_batch("ROLLBACK").unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert_eq!(update.error.unwrap().detail, second);
        assert_eq!(loads(directory.path()), 3);
    }

    #[test]
    fn first_launch_without_provider_or_existing_library() {
        let home = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "library::tests::clean_first_launch_child",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", home.path())
            .env("AGENT_INTERFACE_FIRST_LAUNCH_FIXTURE", "1")
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            home.path()
                .join(".local/share/agent-interface/layouts.db")
                .is_file()
        );
    }

    #[test]
    fn clean_first_launch_child() {
        if std::env::var_os("AGENT_INTERFACE_FIRST_LAUNCH_FIXTURE").is_none() {
            return;
        }
        let (wake, woken) = mpsc::channel();
        let library = Library::start(None, move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert!(update.error.is_none());
        let document = update.document.unwrap();
        assert_eq!(document.model().workspaces.len(), 1);
        assert!(document.model().workspaces[0].tabs.is_empty());
    }

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
        let healthy_writer = Connection::open(directory.path().join("layouts.db")).unwrap();
        let first = Cell::new(true);
        let library = Library::start(Some(directory.path().into()), move || {
            // Commit after the first successful load, before its next poll.
            // This exercises data_version without an intervening read error.
            if first.replace(false) {
                healthy_writer
                    .execute(
                        "UPDATE terminal_layout SET value = ?1",
                        [valid.replace("Initial", "Healthy")],
                    )
                    .unwrap();
            }
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            library
                .updates
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .error
                .is_none()
        );
        // Leave the valid document pending across two distinct failures. Hold
        // the writer lock until the worker observes it, so commit timing cannot
        // decide whether the first error is a lock or an invalid document.
        writer
            .execute_batch("BEGIN EXCLUSIVE; UPDATE terminal_layout SET value = '{}'")
            .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            library
                .updates
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .detail,
            "database is locked"
        );
        writer.execute_batch("COMMIT").unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert_eq!(
            update.document.unwrap().model().workspaces[0].name,
            "Healthy"
        );
        assert_eq!(
            update.error.as_ref().unwrap().detail,
            "layout.version: unsupported version"
        );
        let quiet = woken.recv_timeout(Duration::from_millis(600));
        assert!(
            matches!(quiet, Err(mpsc::RecvTimeoutError::Timeout)),
            "expected no repeated error notification; first error: {:?}, receive: {quiet:?}, next update: {:?}",
            update.error,
            library.take_update()
        );
        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();
        writer
            .execute(
                "UPDATE terminal_layout SET value = ?1",
                [valid.replace("Initial", "Changed")],
            )
            .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            library
                .updates
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .detail,
            "database is locked"
        );
        writer.execute_batch("COMMIT").unwrap();
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
        let library = Library::start(Some(missing.clone()), move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        let update = library.take_update().unwrap();
        assert!(
            update.document.unwrap().model().workspaces[0]
                .tabs
                .is_empty()
        );
        assert!(update.error.is_none());
        drop(library);
        assert!(missing.join("layouts.db").is_file());
    }
}
