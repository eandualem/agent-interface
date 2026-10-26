//! Opt-in structured diagnostics. Producers never wait for disk I/O.
//!
//! Instrument lifecycle boundaries, not terminal bytes, user input, HTTP bodies
//! or credentials. The app owns the subscriber; libraries only emit events.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
};

use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::{ErrorCounter, WorkerGuard};
use tracing_subscriber::{Registry, filter::Targets, layer::SubscriberExt, reload};

/// Saved verbosity. Builds and environment variables never enable verbose logs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Level {
    /// Warnings and errors only.
    #[default]
    Warnings,
    /// Lifecycle events and state changes.
    Debug,
    /// Also record individual operation timings.
    Trace,
}

impl Level {
    /// Settings labels and stable configuration values.
    pub const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Warnings, "warn", "Warnings"),
        (Self::Debug, "debug", "Debug"),
        (Self::Trace, "trace", "Trace"),
    ];

    /// Configuration value for this verbosity.
    pub fn name(self) -> &'static str {
        Self::ALL[self as usize].1
    }

    fn filter(self) -> Targets {
        let level = match self {
            Self::Warnings => LevelFilter::WARN,
            Self::Debug => LevelFilter::DEBUG,
            Self::Trace => LevelFilter::TRACE,
        };
        // Third-party HTTP/UI log records can contain user data. Only explicitly
        // instrumented product targets are enabled, including at Trace.
        Targets::new()
            .with_target("tmux_workspaces_app", level)
            .with_target("tmux_workspaces_tmux", level)
            .with_target("tmux_workspaces_roster", level)
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(0);
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static START_ERROR: Mutex<Option<String>> = Mutex::new(None);

struct Runtime {
    filter: reload::Handle<Targets, Registry>,
    directory: PathBuf,
    health: Arc<Health>,
    dropped: ErrorCounter,
}

#[derive(Default)]
struct Health {
    failed: AtomicU64,
    last_error: Mutex<Option<String>>,
}

/// Keep the background writer alive until the native application exits.
pub struct Guard {
    _worker: WorkerGuard,
}

impl Drop for Guard {
    fn drop(&mut self) {
        tracing::debug!(event = "application_stopped", "Application stopped");
        if let Some(problem) = problem() {
            eprintln!("tmux-workspaces diagnostics: {problem}");
        }
        // WorkerGuard drains the bounded queue after this method returns.
    }
}

/// Initialize local logs for the desktop process. Failures remain visible in
/// Settings and stderr, while the desktop can still be used.
pub fn start(level: Level) -> Option<Guard> {
    match initialize(level) {
        Ok(guard) => Some(guard),
        Err(error) => {
            let message = format!("Logging unavailable: {error}");
            eprintln!("tmux-workspaces: {message}");
            *START_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(message);
            None
        }
    }
}

fn initialize(level: Level) -> io::Result<Guard> {
    let directory = directory().ok_or_else(|| io::Error::other("no log directory"))?;
    let health = Arc::new(Health::default());
    let writer = RotatingWriter::new(directory.clone(), Arc::clone(&health))?;
    let (writer, worker) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(1024)
        .lossy(true)
        .thread_name("diagnostic log writer")
        .finish(writer);
    let dropped = writer.error_counter();
    let (filter, handle) = reload::Layer::new(level.filter());
    let subscriber = Registry::default().with(filter).with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_thread_ids(true)
            .with_writer(writer),
    );
    tracing::subscriber::set_global_default(subscriber).map_err(io::Error::other)?;
    RUNTIME
        .set(Runtime {
            filter: handle,
            directory,
            health,
            dropped,
        })
        .map_err(|_| io::Error::other("logging already initialized"))?;
    LEVEL.store(level as u8, Ordering::Relaxed);
    tracing::debug!(
        event = "application_started",
        version = env!("CARGO_PKG_VERSION"),
        source = env!("BENCH_REVISION"),
        process_id = std::process::id(),
        platform = std::env::consts::OS,
        level = level.name(),
        "Application started"
    );
    Ok(Guard { _worker: worker })
}

/// Change filtering without restarting the process or touching the writer.
pub fn set_level(level: Level) {
    if let Some(runtime) = RUNTIME.get()
        && let Err(error) = runtime.filter.reload(level.filter())
    {
        *START_ERROR.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(format!("Cannot change log level: {error}"));
        return;
    }
    let previous = LEVEL.swap(level as u8, Ordering::Relaxed);
    if previous != level as u8 {
        tracing::debug!(
            event = "logging_changed",
            level = level.name(),
            version = env!("CARGO_PKG_VERSION"),
            source = env!("BENCH_REVISION"),
            process_id = std::process::id(),
            "Log level changed"
        );
    }
}

/// Currently selected verbosity, including before initialization.
pub fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        1 => Level::Debug,
        2 => Level::Trace,
        _ => Level::Warnings,
    }
}

/// Native log location, displayed in Settings. No directory is created here.
pub fn directory() -> Option<PathBuf> {
    if let Some(runtime) = RUNTIME.get() {
        return Some(runtime.directory.clone());
    }
    #[cfg(target_os = "macos")]
    {
        crate::library::location::user_home(None)
            .map(|home| home.join("Library/Logs/Agent Interface"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                crate::library::location::user_home(None).map(|home| home.join(".local/state"))
            })
            .map(|state| state.join("tmux-workspaces/logs"))
    }
}

/// Logging failures and dropped records are never silently reported as success.
pub fn problem() -> Option<String> {
    if let Some(error) = START_ERROR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Some(error);
    }
    let runtime = RUNTIME.get()?;
    let dropped = runtime.dropped.dropped_lines();
    let failed = runtime.health.failed.load(Ordering::Relaxed);
    (dropped != 0 || failed != 0).then(|| {
        let error = runtime
            .health
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        format!(
            "Logs incomplete: {dropped} queued records dropped, {failed} writes failed. {}",
            error
                .as_deref()
                .unwrap_or("The log queue reached capacity.")
        )
    })
}

const FILE_BYTES: u64 = 4 * 1024 * 1024;
const FILE_COUNT: usize = 4;
const RECORD_BYTES: usize = 16 * 1024;

// Only the background worker enters this writer. An advisory lock coordinates
// rotation between app processes; no producer waits for the filesystem or lock.
struct RotatingWriter {
    directory: PathBuf,
    health: Arc<Health>,
    file_bytes: u64,
}

impl RotatingWriter {
    fn new(directory: PathBuf, health: Arc<Health>) -> io::Result<Self> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
        let metadata = directory.symlink_metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != uzers::get_current_uid()
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::other(
                "log directory must be private and owned by this user",
            ));
        }
        private_file(&directory.join("writer.lock"))?;
        private_file(&directory.join("events.jsonl"))?;
        Ok(Self {
            directory,
            health,
            file_bytes: FILE_BYTES,
        })
    }

    fn write_record(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > RECORD_BYTES || bytes.len() as u64 > self.file_bytes {
            return Err(io::Error::other("diagnostic record exceeds size limit"));
        }
        let lock = private_file(&self.directory.join("writer.lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        let result = self.write_locked(bytes);
        // A concurrently spawned child can briefly inherit the descriptor
        // before exec. Explicit unlock releases the shared lock immediately;
        // relying on close can leave it held by that inherited descriptor.
        lock.unlock()?;
        result
    }

    fn write_locked(&self, bytes: &[u8]) -> io::Result<()> {
        let path = self.directory.join("events.jsonl");
        let mut file = private_file(&path)?;
        if file.metadata()?.len() + bytes.len() as u64 > self.file_bytes {
            drop(file);
            for index in (1..FILE_COUNT).rev() {
                let destination = self.directory.join(format!("events.{index}.jsonl"));
                let source = if index == 1 {
                    path.clone()
                } else {
                    self.directory.join(format!("events.{}.jsonl", index - 1))
                };
                match std::fs::rename(source, destination) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            file = private_file(&path)?;
        }
        file.write_all(bytes)
    }
}

fn private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uzers::get_current_uid()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(io::Error::other(
            "log must be a private regular file owned by this user",
        ));
    }
    Ok(file)
}

impl Write for RotatingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.write_record(bytes) {
            Ok(()) => Ok(bytes.len()),
            Err(error) => {
                self.health.failed.fetch_add(1, Ordering::Relaxed);
                *self
                    .health
                    .last_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_files_and_oversized_records_fail_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("untouched");
        std::fs::write(&outside, b"untouched").unwrap();
        let directory = root.path().join("logs");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        std::os::unix::fs::symlink(&outside, directory.join("events.jsonl")).unwrap();
        assert!(RotatingWriter::new(directory.clone(), Arc::default()).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
        std::fs::remove_file(directory.join("events.jsonl")).unwrap();

        let health = Arc::new(Health::default());
        let mut writer = RotatingWriter::new(directory.clone(), Arc::clone(&health)).unwrap();
        assert!(writer.write_all(&vec![b'x'; RECORD_BYTES + 1]).is_err());
        assert_eq!(health.failed.load(Ordering::Relaxed), 1);
        assert_eq!(directory.join("events.jsonl").metadata().unwrap().len(), 0);
    }

    #[test]
    fn levels_filter_product_events_and_exclude_dependency_payloads() {
        let root = tempfile::tempdir().unwrap();
        let health = Arc::new(Health::default());
        let writer = RotatingWriter::new(root.path().join("logs"), Arc::clone(&health)).unwrap();
        let (writer, guard) = tracing_appender::non_blocking(writer);
        let (filter, handle) = reload::Layer::new(Level::Warnings.filter());
        let subscriber = Registry::default()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json().with_writer(writer));
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(event = "hidden_default", "hidden");
            tracing::warn!(event = "expected_warning", "warning");
            handle.reload(Level::Debug.filter()).unwrap();
            tracing::debug!(event = "expected_debug", "debug");
            tracing::trace!(event = "hidden_trace", "hidden");
            handle.reload(Level::Trace.filter()).unwrap();
            tracing::trace!(event = "expected_trace", "trace");
            tracing::error!(target: "http_client", "fixture-private-payload");
        });
        drop(guard);
        let text = std::fs::read_to_string(root.path().join("logs/events.jsonl")).unwrap();
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            health.failed.load(Ordering::Relaxed),
            0,
            "{:?}",
            health.last_error.lock().unwrap()
        );
        assert_eq!(events.len(), 3, "{text}");
        assert_eq!(events[0]["fields"]["event"], "expected_warning");
        assert_eq!(events[1]["fields"]["event"], "expected_debug");
        assert_eq!(events[2]["fields"]["event"], "expected_trace");
        assert!(!text.contains("private-payload"));
    }

    #[test]
    fn rotation_is_bounded_and_writer_failures_are_observable() {
        let root = tempfile::tempdir().unwrap();
        let health = Arc::new(Health::default());
        let mut writer =
            RotatingWriter::new(root.path().join("logs"), Arc::clone(&health)).unwrap();
        writer.file_bytes = 32;
        for n in 0..20 {
            writer
                .write_all(format!("{{\"event\":{n}}}\n").as_bytes())
                .unwrap();
        }
        for index in 0..FILE_COUNT {
            let name = if index == 0 {
                "events.jsonl".into()
            } else {
                format!("events.{index}.jsonl")
            };
            let path = writer.directory.join(name);
            assert!(path.metadata().unwrap().len() <= 32);
            for line in std::fs::read_to_string(path).unwrap().lines() {
                assert!(serde_json::from_str::<serde_json::Value>(line).is_ok());
            }
        }
        assert_eq!(
            std::fs::read_dir(&writer.directory).unwrap().count(),
            FILE_COUNT + 1
        );
        let lock = private_file(&writer.directory.join("writer.lock")).unwrap();
        lock.lock().unwrap();
        assert!(writer.write_all(b"blocked\n").is_err());
        assert_eq!(health.failed.load(Ordering::Relaxed), 1);
        lock.unlock().unwrap();
        writer.write_all(b"available\n").unwrap();
    }
}
