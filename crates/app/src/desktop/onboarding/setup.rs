//! Bounded local setup protocol. Requests contain no credentials; the child
//! inherits the caller's authority and never substitutes an operator identity.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{FileExt, OpenOptionsExt},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const OUTPUT_LIMIT: usize = 1024 * 1024;
const LINE_LIMIT: usize = 64 * 1024;
const REQUEST_LIMIT: usize = 32 * 1024;
// Room for consumer metadata added to records written by older app versions.
const SAVED_LIMIT: usize = REQUEST_LIMIT + 128;
const EVENT_LIMIT: u64 = 1024;

// Avoid blocking the UI on a FIFO or following a substituted metadata file.
fn regular_file(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("Expected a regular file"));
    }
    Ok(file)
}

#[derive(Clone, Deserialize)]
pub struct Runtime {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    #[serde(default)]
    pub setup_supported: bool,
    pub models: Vec<String>,
    pub authentication: String,
    #[serde(default)]
    pub setup_compatibility: Option<Compatibility>,
}

#[derive(Clone, Deserialize)]
pub struct Compatibility {
    pub compatible: bool,
    pub message: String,
    pub path: Option<String>,
    pub version: Option<String>,
}

impl Runtime {
    pub fn can_setup(&self) -> bool {
        self.available
            && self.setup_supported
            && self.id != "shell"
            && self
                .setup_compatibility
                .as_ref()
                .is_none_or(|check| check.compatible)
    }
}

#[derive(Deserialize)]
struct Catalog {
    schema_version: u32,
    items: Vec<Runtime>,
}

/// Agent creation input is saved before execution so an interrupted app can
/// retry the exact same operation rather than allocate another request ID.
#[derive(Clone, Deserialize, Serialize)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    pub action: String,
    pub data_dir: std::path::PathBuf,
    pub hooks: Vec<String>,
    pub agent: Agent,
    pub start_greeting: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Agent {
    pub name: String,
    pub directory: std::path::PathBuf,
    pub create_directory: bool,
    pub runtime: String,
    pub model: Option<String>,
    pub brief: Brief,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Saved {
    #[serde(default)]
    pub cancelling: bool,
    pub path_env: String,
    pub executable: std::path::PathBuf,
    pub request: Request,
}

impl Saved {
    /// Different provider installations must never share a pending operation.
    pub fn path(settings: &Path, data_dir: &Path) -> std::path::PathBuf {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let installation = Sha256::digest(data_dir.as_os_str().as_bytes());
        settings.with_file_name(format!("onboarding-agent-{installation:x}.json"))
    }

    pub fn load(path: &Path) -> Result<Option<Self>, &'static str> {
        let file = match regular_file(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("The saved setup request cannot be read."),
        };
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take((SAVED_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "The saved setup request cannot be read.")?;
        if bytes.len() > SAVED_LIMIT {
            return Err("The saved setup request is too large.");
        }
        let saved: Self = serde_json::from_slice(&bytes)
            .map_err(|_| "The saved setup request cannot be read.")?;
        if saved.request.schema_version != 1
            || saved.request.action != "first_agent"
            || !saved.executable.is_absolute()
            || !saved.request.data_dir.is_absolute()
            || !saved.request.agent.directory.is_absolute()
            || saved.request.request_id.is_empty()
        {
            return Err("The saved setup request is unsupported.");
        }
        Ok(Some(saved))
    }

    /// Only abandon a provider-confirmed preflight failure. Serialize this with
    /// creation so another setup window cannot have its newer request archived.
    #[cfg(test)]
    fn archive(&self, path: &Path) -> Result<(), &'static str> {
        let _guard = request_lock(path)?;
        self.verify_current(path)?;
        self.archive_locked(path, None)
    }

    fn archive_locked(
        &self,
        path: &Path,
        confirmation: Option<&Event>,
    ) -> Result<(), &'static str> {
        let archive =
            path.with_file_name(format!("onboarding-agent-{}.json", uuid::Uuid::new_v4()));
        let parent = path
            .parent()
            .ok_or("Cannot retain the previous setup request.")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Cannot retain the previous setup request.")?;
        let bytes = serde_json::to_vec(&serde_json::json!({"saved":self,"result":confirmation}))
            .map_err(|_| "Cannot retain the previous setup request.")?;
        file.write_all(&bytes)
            .map_err(|_| "Cannot retain the previous setup request.")?;
        file.as_file()
            .sync_all()
            .map_err(|_| "Cannot retain the previous setup request.")?;
        file.persist_noclobber(archive)
            .map_err(|_| "Cannot retain the previous setup request.")?;
        std::fs::remove_file(path).map_err(|_| "Cannot retire the previous setup request.")
    }

    fn verify_current(&self, path: &Path) -> Result<(), &'static str> {
        let disk = Self::load(path)?.ok_or("Saved setup changed. Reopen setup.")?;
        if serde_json::to_vec(&disk).ok() != serde_json::to_vec(self).ok() {
            return Err("Saved setup changed. Reopen setup.");
        }
        Ok(())
    }

    /// Serialize execution with retirement, not only with writing the form.
    /// A stale window must not replay an attempt after another window changed it.
    pub fn run_saved(&self, path: &Path, emit: impl FnMut(Event)) -> Result<Event, &'static str> {
        let _guard = request_lock(path)?;
        self.verify_current(path)?;
        if self.cancelling {
            return Err("Changing tools is pending. Retry changing tools to finish safely.");
        }
        run(&self.executable, &self.request, &self.path_env, emit)
    }

    /// Record the user's intent before contacting the provider. Even after a
    /// crash, Resume must retry cancellation rather than start the old tool.
    pub fn begin_cancel(&mut self, path: &Path) -> Result<(), &'static str> {
        let _guard = request_lock(path)?;
        self.verify_current(path)?;
        let mut next = self.clone();
        next.cancelling = true;
        next.replace_locked(path)?;
        *self = next;
        Ok(())
    }

    fn replace_locked(&self, path: &Path) -> Result<(), &'static str> {
        let parent = path.parent().ok_or("Cannot save setup progress.")?;
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot save the tool change.")?;
        let bytes = serde_json::to_vec(self).map_err(|_| "Cannot save the tool change.")?;
        file.write_all(&bytes)
            .map_err(|_| "Cannot save the tool change.")?;
        file.as_file()
            .sync_all()
            .map_err(|_| "Cannot save the tool change.")?;
        file.persist(path)
            .map_err(|_| "Cannot save the tool change.")?;
        Ok(())
    }

    pub fn cancel_saved(
        &self,
        path: &Path,
        executable: &Path,
        path_env: &str,
    ) -> Result<Event, &'static str> {
        let _guard = request_lock(path)?;
        self.verify_current(path)?;
        if !self.cancelling {
            return Err("Save the tool change before cancelling setup.");
        }
        let mut request = self.request.clone();
        request.action = "cancel_first_agent".into();
        // Cancellation can use the updated engine for the same installation;
        // normal setup replay still uses the saved launcher and environment.
        let result = run(executable, &request, path_env, |_| {})?;
        if result.cancelled(&request.request_id) {
            self.archive_locked(path, Some(&result))?;
        } else if result.status == "action_required"
            && result.code == "cancellation_unsafe"
            && result.retry == "none"
        {
            // The provider definitively refused cancellation; preserve the
            // original operation and let the user explicitly resume it.
            let mut next = self.clone();
            next.cancelling = false;
            next.replace_locked(path)?;
        }
        Ok(result)
    }

    /// Never replace a pending operation, including one from another window.
    pub fn create(&self, path: &Path) -> Result<(), &'static str> {
        let parent = path.parent().ok_or("Cannot save setup progress.")?;
        std::fs::create_dir_all(parent).map_err(|_| "Cannot save setup progress.")?;
        let _guard = request_lock(path)?;
        let bytes = serde_json::to_vec(self).map_err(|_| "Cannot save setup progress.")?;
        if bytes.len() > REQUEST_LIMIT {
            return Err("The agent instructions are too long.");
        }
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot save setup progress.")?;
        file.write_all(&bytes)
            .map_err(|_| "Cannot save setup progress.")?;
        file.as_file()
            .sync_all()
            .map_err(|_| "Cannot save setup progress.")?;
        file.persist_noclobber(path)
            .map_err(|_| "A saved setup request already exists. Reopen setup to resume it.")?;
        Ok(())
    }
}

struct RequestLock(File);

impl Drop for RequestLock {
    fn drop(&mut self) {
        // A concurrently spawned child can briefly share the open file
        // description before exec closes it. Closing our descriptor alone
        // would leave the lock held by that child until then.
        let _ = self.0.unlock();
    }
}

fn request_lock(path: &Path) -> Result<RequestLock, &'static str> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path.with_extension("lock"))
        .map_err(|_| "Cannot lock setup progress.")?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("Cannot lock setup progress.");
    }
    file.try_lock()
        .map_err(|_| "Another setup window is updating this request.")?;
    Ok(RequestLock(file))
}

/// Installer activation is only a launcher lookup for the same installation.
/// It neither supplies credentials nor overrides an explicitly selected provider.
pub fn active_engine(data_dir: &Path) -> Option<std::path::PathBuf> {
    #[derive(Deserialize)]
    struct Activation {
        schema_version: u32,
        payload_id: String,
        executable: std::path::PathBuf,
        data_dir: std::path::PathBuf,
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let root = home.join(".local/lib/agent-interface-engine");
    let file = regular_file(&root.join("active.json")).ok()?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    let active: Activation = serde_json::from_slice(&bytes).ok()?;
    if active.schema_version != 1
        || !active.executable.is_absolute()
        || !active.data_dir.is_absolute()
        || active.payload_id.is_empty()
        || active.payload_id.len() > 128
        || active.payload_id == "."
        || active.payload_id == ".."
        || !active
            .payload_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        || active.data_dir.canonicalize().ok()? != data_dir.canonicalize().ok()?
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let payload = root.join(active.payload_id).canonicalize().ok()?;
    if payload.parent()? != root {
        return None;
    }
    let launcher = payload.join("bin/backbone").canonicalize().ok()?;
    (launcher.starts_with(&payload) && active.executable.canonicalize().ok()? == launcher)
        .then_some(launcher)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Brief {
    Template { template: String },
    Text { text: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Event {
    pub schema_version: u32,
    pub request_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub retry: String,
    #[serde(default)]
    pub completed_steps: Vec<String>,
    #[serde(default)]
    pub data: Value,
}

impl Event {
    pub fn cancelled(&self, request_id: &str) -> bool {
        self.action == "cancel_first_agent"
            && self.request_id == request_id
            && self.status == "completed"
            && self.code == "request_cancelled"
            && self.retry == "new_request"
            && self.completed_steps.is_empty()
            && self
                .data
                .get("cancelled_request_id")
                .and_then(Value::as_str)
                == Some(request_id)
            && self
                .data
                .get("can_start_new_request")
                .and_then(Value::as_bool)
                == Some(true)
    }
}

/// A missing or contradictory final result cannot establish what was changed.
pub const UNKNOWN: &str =
    "Setup's outcome is unknown. Retry the saved request to check it; do not create another agent.";

struct Stream {
    request_id: String,
    action: String,
    sequence: u64,
    pending: Vec<u8>,
    total: usize,
    result: Option<Event>,
}

impl Stream {
    fn new(request_id: &str) -> Self {
        Self {
            request_id: request_id.into(),
            action: "first_agent".into(),
            sequence: 0,
            pending: Vec::new(),
            total: 0,
            result: None,
        }
    }

    fn push(&mut self, bytes: &[u8], emit: &mut impl FnMut(Event)) -> Result<(), &'static str> {
        self.total = self.total.checked_add(bytes.len()).ok_or(UNKNOWN)?;
        if self.total > OUTPUT_LIMIT {
            return Err(UNKNOWN);
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            if end > LINE_LIMIT {
                return Err(UNKNOWN);
            }
            let line: Vec<_> = self.pending.drain(..=end).collect();
            self.line(&line, emit)?;
        }
        if self.pending.len() > LINE_LIMIT {
            return Err(UNKNOWN);
        }
        Ok(())
    }

    fn line(&mut self, line: &[u8], emit: &mut impl FnMut(Event)) -> Result<(), &'static str> {
        let event: Event = serde_json::from_slice(line).map_err(|_| UNKNOWN)?;
        if event.schema_version != 1
            || event.request_id != self.request_id
            || event.seq <= self.sequence
            || event.seq > EVENT_LIMIT
            || self.result.is_some()
        {
            return Err(UNKNOWN);
        }
        match event.kind.as_str() {
            "step"
                if matches!(
                    event.status.as_str(),
                    "running" | "completed" | "skipped" | "failed"
                ) => {}
            "result"
                if event.action == self.action
                    && matches!(
                        event.status.as_str(),
                        "completed" | "action_required" | "failed"
                    )
                    && !event.code.is_empty()
                    && matches!(
                        event.retry.as_str(),
                        "same_request" | "new_request" | "none"
                    ) =>
            {
                self.result = Some(event.clone());
            }
            _ => return Err(UNKNOWN),
        }
        self.sequence = event.seq;
        emit(event);
        Ok(())
    }

    fn finish(
        mut self,
        exit: ExitStatus,
        emit: &mut impl FnMut(Event),
    ) -> Result<Event, &'static str> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line, emit)?;
        }
        let result = self.result.ok_or(UNKNOWN)?;
        let consistent = match result.status.as_str() {
            "completed" => exit.code() == Some(0),
            "action_required" => exit.code() == Some(3),
            "failed" => matches!(exit.code(), Some(1 | 2)),
            _ => false,
        };
        consistent.then_some(result).ok_or(UNKNOWN)
    }
}

/// Reads use positional offsets, never the cursor shared with child writers.
fn drain(
    file: &File,
    offset: &mut u64,
    consume: &mut impl FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut bytes = [0; 8192];
    loop {
        match file.read_at(&mut bytes, *offset) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                *offset += n as u64;
                consume(&bytes[..n])?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(UNKNOWN),
        }
    }
}

/// The engine must emit a bounded JSON stream. No shell evaluates the path or
/// request, and no stderr diagnostics or environment values enter the UI.
pub fn run(
    executable: &Path,
    request: &Request,
    path_env: &str,
    mut emit: impl FnMut(Event),
) -> Result<Event, &'static str> {
    run_with_timeout(
        executable,
        request,
        path_env,
        &mut emit,
        Duration::from_secs(300),
    )
}

fn run_with_timeout(
    executable: &Path,
    request: &Request,
    path_env: &str,
    mut emit: impl FnMut(Event),
    timeout: Duration,
) -> Result<Event, &'static str> {
    let bytes = serde_json::to_vec(request).map_err(|_| "Cannot prepare setup request.")?;
    let limit = REQUEST_LIMIT
        + if request.action == "cancel_first_agent" {
            "cancel_".len()
        } else {
            0
        };
    if bytes.len() > limit {
        return Err("The agent instructions are too long.");
    }
    let mut input = tempfile::NamedTempFile::new().map_err(|_| "Cannot prepare setup request.")?;
    input
        .write_all(&bytes)
        .map_err(|_| "Cannot prepare setup request.")?;
    let output = tempfile::tempfile().map_err(|_| "Cannot capture setup progress.")?;
    let mut child = Command::new(executable)
        .env("PATH", path_env)
        .args(["setup", "--file"])
        .arg(input.path())
        .arg("--jsonl")
        .stdin(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|_| "Cannot capture setup progress.")?,
        )
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot start the setup engine.")?;
    let mut stream = Stream::new(&request.request_id);
    stream.action = request.action.clone();
    let mut offset = 0;
    let deadline = Instant::now() + timeout;
    let result = loop {
        if let Err(error) = drain(&output, &mut offset, &mut |bytes| {
            stream.push(bytes, &mut emit)
        }) {
            break Err(error);
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                if let Err(error) = drain(&output, &mut offset, &mut |bytes| {
                    stream.push(bytes, &mut emit)
                }) {
                    break Err(error);
                }
                break stream.finish(exit, &mut emit).and_then(|result| {
                    if request.action == "cancel_first_agent" && result.status == "completed" {
                        return result
                            .cancelled(&request.request_id)
                            .then_some(result)
                            .ok_or(UNKNOWN);
                    }
                    if result.status == "completed"
                        && (result.code != "agent_ready"
                            || result.data.get("agent_name").and_then(Value::as_str)
                                != Some(&request.agent.name)
                            || result.data.get("ready").and_then(Value::as_str) != Some("ready"))
                    {
                        Err(UNKNOWN)
                    } else {
                        Ok(result)
                    }
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => break Err(UNKNOWN),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn runtimes(executable: &Path, path_env: &str) -> Result<Vec<Runtime>, &'static str> {
    match runtime_catalog(executable, path_env, true) {
        Err(DISCOVERY_FLAG_UNSUPPORTED) => runtime_catalog(executable, path_env, false),
        result => result,
    }
}

const DISCOVERY_FLAG_UNSUPPORTED: &str = "This engine does not support compatibility checks.";

fn runtime_catalog(
    executable: &Path,
    path_env: &str,
    check_setup: bool,
) -> Result<Vec<Runtime>, &'static str> {
    let output = tempfile::tempfile().map_err(|_| "Cannot read available agent tools.")?;
    let mut command = Command::new(executable);
    command.env("PATH", path_env).args(["runtimes", "--json"]);
    if check_setup {
        command.arg("--check-setup");
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|_| "Cannot read available agent tools.")?,
        )
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot start the setup engine.")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut bytes = Vec::new();
    let mut offset = 0;
    let result = loop {
        let mut collect = |part: &[u8]| {
            if bytes.len() + part.len() > OUTPUT_LIMIT {
                return Err("Agent tool discovery returned too much data.");
            }
            bytes.extend_from_slice(part);
            Ok(())
        };
        if let Err(error) = drain(&output, &mut offset, &mut collect) {
            break Err(error);
        }
        match child.try_wait() {
            Ok(Some(exit)) if exit.success() => {
                if let Err(error) = drain(&output, &mut offset, &mut collect) {
                    break Err(error);
                }
                break parse_catalog(&bytes);
            }
            Ok(Some(exit)) if check_setup && exit.code() == Some(2) => {
                break Err(DISCOVERY_FLAG_UNSUPPORTED);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                break Err(
                    "This engine does not provide setup discovery. Update it before creating an agent here.",
                );
            }
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn parse_catalog(bytes: &[u8]) -> Result<Vec<Runtime>, &'static str> {
    let catalog: Catalog =
        serde_json::from_slice(bytes).map_err(|_| "Cannot read agent tool discovery.")?;
    let mut ids = std::collections::HashSet::new();
    if catalog.schema_version != 1
        || catalog.items.len() > 64
        || catalog.items.iter().any(|row| {
            row.id.is_empty()
                || row.id.len() > 64
                || !ids.insert(&row.id)
                || row.display_name.len() > 128
                || row.models.len() > 256
                || row.setup_compatibility.as_ref().is_some_and(|check| {
                    check.message.len() > 4096
                        || check.path.as_ref().is_some_and(|path| path.len() > 4096)
                        || check
                            .version
                            .as_ref()
                            .is_some_and(|version| version.len() > 128)
                })
                || !matches!(row.authentication.as_str(), "unknown" | "not_applicable")
        })
    {
        return Err("Unsupported agent tool discovery.");
    }
    Ok(catalog.items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    const PROGRESS: &str = include_str!("fixtures/first-agent-action-required.jsonl");

    #[test]
    fn provider_fixture_survives_arbitrary_chunks_and_needs_matching_exit() {
        let mut stream = Stream::new("onboarding-first-agent-01");
        let mut events = Vec::new();
        for bytes in PROGRESS.as_bytes().chunks(7) {
            stream.push(bytes, &mut |event| events.push(event)).unwrap();
        }
        let result = stream
            .finish(ExitStatus::from_raw(3 << 8), &mut |_| {})
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(result.code, "runtime_permission");
        assert_eq!(result.retry, "same_request");
        assert_eq!(result.data["agent_name"], "guide");
        let mut stream = Stream::new("onboarding-first-agent-01");
        stream.push(PROGRESS.as_bytes(), &mut |_| {}).unwrap();
        assert!(stream.finish(ExitStatus::from_raw(0), &mut |_| {}).is_err());
    }

    #[test]
    fn wrong_request_repeated_sequence_and_post_result_output_are_unknown() {
        let mut wrong = Stream::new("another-request");
        assert!(wrong.push(PROGRESS.as_bytes(), &mut |_| {}).is_err());
        let first = PROGRESS.lines().next().unwrap().to_owned() + "\n";
        let mut repeated = Stream::new("onboarding-first-agent-01");
        repeated.push(first.as_bytes(), &mut |_| {}).unwrap();
        assert!(repeated.push(first.as_bytes(), &mut |_| {}).is_err());
        let mut done = Stream::new("onboarding-first-agent-01");
        done.push(PROGRESS.as_bytes(), &mut |_| {}).unwrap();
        assert!(done.push(first.as_bytes(), &mut |_| {}).is_err());
    }

    #[test]
    fn missing_result_and_unbounded_line_are_unknown() {
        let stream = Stream::new("request");
        assert!(stream.finish(ExitStatus::from_raw(0), &mut |_| {}).is_err());
        let mut stream = Stream::new("request");
        assert!(
            stream
                .push(&vec![b'x'; LINE_LIMIT + 1], &mut |_| {})
                .is_err()
        );
    }

    #[test]
    fn discovery_uses_provider_fixture_without_claiming_login() {
        let rows = parse_catalog(include_bytes!("fixtures/runtimes-example.json")).unwrap();
        assert_eq!(rows[0].id, "codex");
        assert!(rows[0].available);
        assert!(rows[0].can_setup());
        assert_eq!(rows[0].authentication, "unknown");
    }

    #[test]
    fn discovery_requires_explicit_setup_support_and_an_available_agent_tool() {
        let mut catalog: Value =
            serde_json::from_str(include_str!("fixtures/runtimes-example.json")).unwrap();
        catalog["items"][0]
            .as_object_mut()
            .unwrap()
            .remove("setup_supported");
        let read = |value: &Value| parse_catalog(&serde_json::to_vec(value).unwrap()).unwrap();
        assert!(!read(&catalog)[0].can_setup());
        catalog["items"][0]["setup_supported"] = Value::Bool(false);
        assert!(!read(&catalog)[0].can_setup());
        catalog["items"][0]["setup_supported"] = Value::Bool(true);
        assert!(read(&catalog)[0].can_setup());
        catalog["items"][0]["available"] = Value::Bool(false);
        assert!(!read(&catalog)[0].can_setup());
        catalog["items"][0]["available"] = Value::Bool(true);
        catalog["items"][0]["id"] = Value::String("shell".into());
        assert!(!read(&catalog)[0].can_setup());
    }

    #[test]
    fn compatibility_checks_disable_old_tools_without_hiding_the_diagnosis() {
        let mut catalog: Value =
            serde_json::from_str(include_str!("fixtures/runtimes-example.json")).unwrap();
        catalog["items"][0]["setup_compatibility"] = serde_json::json!({
            "compatible":false,"reason":"version_too_old","message":"Update this tool.",
            "path":"/old/bin/opencode","version":"1.19.5","version_error":null});
        let rows = parse_catalog(&serde_json::to_vec(&catalog).unwrap()).unwrap();
        assert!(!rows[0].can_setup());
        assert_eq!(
            rows[0]
                .setup_compatibility
                .as_ref()
                .unwrap()
                .version
                .as_deref(),
            Some("1.19.5")
        );
    }

    #[test]
    fn discovery_falls_back_only_for_an_unsupported_cli_option() {
        let root = tempfile::tempdir().unwrap();
        let catalog = include_str!("fixtures/runtimes-example.json");
        let engine = fake_engine(
            root.path(),
            &format!(
                "printf '%s\\n' \"$*\" >> \"$0.calls\"\n[ \"$3\" != --check-setup ] || exit 2\ncat <<'CATALOG'\n{catalog}\nCATALOG"
            ),
        );
        assert!(runtimes(&engine, "/usr/bin:/bin").is_ok());
        let calls = engine.with_file_name("fake engine.calls");
        assert_eq!(
            std::fs::read_to_string(&calls).unwrap(),
            "runtimes --json --check-setup\nruntimes --json\n"
        );
        std::fs::remove_file(&calls).unwrap();
        fake_engine(
            root.path(),
            "printf '%s\\n' \"$*\" >> \"$0.calls\"\nprintf 'invalid json'",
        );
        assert!(runtimes(&engine, "/usr/bin:/bin").is_err());
        assert_eq!(std::fs::read_to_string(calls).unwrap().lines().count(), 1);
    }

    fn request() -> Request {
        serde_json::from_str(include_str!("fixtures/first-agent-request.json")).unwrap()
    }

    fn fake_engine(root: &Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake engine");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn cancellation_result() -> Value {
        serde_json::json!({"schema_version":1,"request_id":request().request_id,
            "seq":1,"type":"result","action":"cancel_first_agent",
            "status":"completed","code":"request_cancelled","retry":"new_request",
            "data":{"cancelled_request_id":request().request_id,"can_start_new_request":true}})
    }

    #[test]
    fn tool_change_survives_restart_and_retires_only_confirmed_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("saved.json");
        let engine = fake_engine(
            root.path(),
            &format!(
                "cp \"$3\" \"$0.request\"\nprintf '%s\\n' '{}'",
                cancellation_result()
            ),
        );
        let mut saved = Saved {
            cancelling: false,
            executable: engine.clone(),
            path_env: "/usr/bin:/bin".into(),
            request: request(),
        };
        saved.create(&path).unwrap();
        let stale = saved.clone();
        saved.begin_cancel(&path).unwrap();
        assert!(stale.run_saved(&path, |_| {}).is_err());
        let restored = Saved::load(&path).unwrap().unwrap();
        assert!(restored.cancelling);
        assert!(restored.run_saved(&path, |_| {}).is_err());
        assert!(!engine.with_file_name("fake engine.request").exists());
        let result = restored
            .cancel_saved(&path, &engine, "/usr/bin:/bin")
            .unwrap();
        assert!(result.cancelled(&request().request_id));
        assert!(Saved::load(&path).unwrap().is_none());
        let archive = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("onboarding-agent-")
            })
            .unwrap();
        let retained: Value = serde_json::from_slice(&std::fs::read(archive).unwrap()).unwrap();
        assert_eq!(retained["result"]["code"], "request_cancelled");
        assert_eq!(
            retained["saved"]["request"],
            serde_json::to_value(request()).unwrap()
        );
        let mut submitted: Value = serde_json::from_slice(
            &std::fs::read(engine.with_file_name("fake engine.request")).unwrap(),
        )
        .unwrap();
        assert_eq!(submitted["action"], "cancel_first_agent");
        submitted["action"] = Value::String("first_agent".into());
        assert_eq!(submitted, serde_json::to_value(request()).unwrap());
        assert!(restored.run_saved(&path, |_| {}).is_err());
    }

    #[test]
    fn unsafe_missing_or_mismatched_cancellation_never_unlocks_the_form() {
        let mut wrong_id = cancellation_result();
        wrong_id["data"]["cancelled_request_id"] = "someone-else".into();
        let mut wrong_action = cancellation_result();
        wrong_action["action"] = "first_agent".into();
        let mut unsafe_result = cancellation_result();
        unsafe_result["status"] = "action_required".into();
        unsafe_result["code"] = "cancellation_unsafe".into();
        unsafe_result["retry"] = "none".into();
        unsafe_result["data"] = serde_json::json!({});
        for (result, exit) in [
            (wrong_id, 0),
            (wrong_action, 0),
            (unsafe_result, 3),
            (Value::Null, 0),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("saved.json");
            let engine = fake_engine(
                root.path(),
                &format!("printf '%s\\n' '{result}'\nexit {exit}"),
            );
            let mut saved = Saved {
                cancelling: false,
                executable: engine,
                path_env: "/usr/bin:/bin".into(),
                request: request(),
            };
            saved.create(&path).unwrap();
            saved.begin_cancel(&path).unwrap();
            let result = saved.cancel_saved(&path, &saved.executable, &saved.path_env);
            assert!(!result.is_ok_and(|event| event.cancelled(&request().request_id)));
            let restored = Saved::load(&path).unwrap().unwrap();
            assert_eq!(restored.request.request_id, request().request_id);
            assert_eq!(restored.cancelling, exit != 3);
        }
    }

    #[test]
    fn maximum_request_and_legacy_saved_record_remain_cancellable() {
        let root = tempfile::tempdir().unwrap();
        let engine = fake_engine(
            root.path(),
            &format!("printf '%s\\n' '{}'", cancellation_result()),
        );
        let mut input = request();
        input.agent.brief = Brief::Text {
            text: String::new(),
        };
        let size = serde_json::to_vec(&input).unwrap().len();
        input.agent.brief = Brief::Text {
            text: "x".repeat(REQUEST_LIMIT - size),
        };
        assert_eq!(serde_json::to_vec(&input).unwrap().len(), REQUEST_LIMIT);
        input.action = "cancel_first_agent".into();
        assert!(
            run(&engine, &input, "/usr/bin:/bin", |_| {})
                .unwrap()
                .cancelled(&input.request_id)
        );

        let path = root.path().join("legacy.json");
        let mut legacy =
            serde_json::json!({"executable":engine,"path_env":"/usr/bin:/bin","request":request()});
        legacy["request"]["agent"]["brief"] = serde_json::json!({"text":""});
        let size = serde_json::to_vec(&legacy).unwrap().len();
        legacy["request"]["agent"]["brief"]["text"] = "x".repeat(REQUEST_LIMIT - size).into();
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mut saved = Saved::load(&path).unwrap().unwrap();
        saved.begin_cancel(&path).unwrap();
        assert!(Saved::load(&path).unwrap().unwrap().cancelling);
    }

    #[test]
    fn saved_operation_is_private_exclusive_and_cannot_archive_a_newer_request() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("saved.json");
        let saved = Saved {
            cancelling: false,
            executable: root.path().join("engine"),
            path_env: "/usr/bin:/bin".into(),
            request: request(),
        };
        saved.create(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(saved.create(&path).is_err());
        let loaded = Saved::load(&path).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&loaded.request).unwrap(),
            serde_json::to_value(request()).unwrap()
        );
        let guard = request_lock(&path).unwrap();
        assert!(saved.archive(&path).is_err());
        assert_eq!(
            saved.run_saved(&path, |_| {}).unwrap_err(),
            "Another setup window is updating this request."
        );
        // Duplicated descriptors share the same lock, as does a child between
        // fork and exec. Dropping the guard must release it even then.
        let inherited = guard.0.try_clone().unwrap();
        drop(guard);
        saved.archive(&path).unwrap();
        drop(inherited);
        let mut newer = saved.clone();
        newer.request.request_id = "newer-request".into();
        newer.create(&path).unwrap();
        assert!(saved.archive(&path).is_err());
        assert_eq!(
            saved.run_saved(&path, |_| {}).unwrap_err(),
            "Saved setup changed. Reopen setup."
        );
        assert_eq!(
            Saved::load(&path).unwrap().unwrap().request.request_id,
            "newer-request"
        );
    }

    #[test]
    fn saved_metadata_rejects_symlinks_directories_and_fifos_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::write(&target, "{}").unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(regular_file(&link).is_err());
        assert!(regular_file(root.path()).is_err());
        let fifo = root.path().join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(regular_file(&fifo).is_err());
    }

    #[test]
    fn child_receives_exact_saved_request_on_retry_and_exit_is_checked() {
        let root = tempfile::tempdir().unwrap();
        let executable = fake_engine(
            root.path(),
            &format!(
                "[ \"$1\" = setup ] && [ \"$2\" = --file ] && [ \"$4\" = --jsonl ] || exit 8\n/bin/cp \"$3\" \"$0.request\"\n/bin/cat <<'EVENTS'\n{PROGRESS}EVENTS\nexit 3"
            ),
        );
        for _ in 0..2 {
            let result = run(&executable, &request(), "/usr/bin:/bin", |_| {}).unwrap();
            assert_eq!(result.code, "runtime_permission");
            let actual: Value = serde_json::from_slice(
                &std::fs::read(executable.with_file_name("fake engine.request")).unwrap(),
            )
            .unwrap();
            assert_eq!(actual, serde_json::to_value(request()).unwrap());
        }
        fake_engine(
            root.path(),
            &format!("/bin/cat <<'EVENTS'\n{PROGRESS}EVENTS\nexit 0"),
        );
        assert_eq!(
            run(&executable, &request(), "/usr/bin:/bin", |_| {}).unwrap_err(),
            UNKNOWN
        );
    }

    #[test]
    fn stalled_child_and_contradictory_success_preserve_unknown_outcome() {
        let root = tempfile::tempdir().unwrap();
        let executable = fake_engine(root.path(), "exec /bin/sleep 10");
        let start = Instant::now();
        assert_eq!(
            run_with_timeout(
                &executable,
                &request(),
                "/usr/bin:/bin",
                |_| {},
                Duration::from_millis(20)
            )
            .unwrap_err(),
            UNKNOWN
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let result = serde_json::json!({"schema_version":1,"request_id":request().request_id,"seq":1,"type":"result","action":"first_agent","status":"completed","code":"agent_ready","retry":"none","data":{"agent_name":"different-agent","ready":"ready"}});
        fake_engine(
            root.path(),
            &format!("/bin/cat <<'EVENTS'\n{result}\nEVENTS\nexit 0"),
        );
        assert_eq!(
            run(&executable, &request(), "/usr/bin:/bin", |_| {}).unwrap_err(),
            UNKNOWN
        );
    }
    #[test]
    fn saved_operations_are_scoped_to_each_installation_and_survive_missing_discovery() {
        let root = tempfile::tempdir().unwrap();
        let settings = root.path().join("settings.toml");
        let mut a = Saved {
            cancelling: false,
            executable: root.path().join("previous engine"),
            path_env: "/old/tools".into(),
            request: request(),
        };
        a.request.data_dir = root.path().join("installation a");
        let mut b = a.clone();
        b.request.data_dir = root.path().join("installation b");
        b.request.request_id = "other-request".into();
        let pa = Saved::path(&settings, &a.request.data_dir);
        let pb = Saved::path(&settings, &b.request.data_dir);
        assert_ne!(pa, pb);
        a.create(&pa).unwrap();
        b.create(&pb).unwrap();
        let restored = Saved::load(&pa).unwrap().unwrap();
        assert_eq!(restored.executable, a.executable);
        assert_eq!(restored.path_env, "/old/tools");
        assert_eq!(
            Saved::load(&pb).unwrap().unwrap().request.request_id,
            "other-request"
        );
    }
}
