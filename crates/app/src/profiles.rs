//! Local operator discovery, verified selection and explicitly prepared profile
//! operations. Discovery and selection never invoke lifecycle or setup actions.

mod invocation_file;
mod recovery;

pub mod controller;
pub mod journal;
pub mod lifecycle;
pub mod selection;
pub mod setup;
pub mod setup_controller;

use agent_interface_roster::profiles::{self, Item, Profile};
use std::{
    ffi::OsString,
    io::Read,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(8);

/// Fixed diagnostics; command output, credentials and provider prose stay private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid executable path or profile identifier.
    Invalid,
    /// The executable or its bounded output could not be accessed.
    Unavailable,
    /// A command exited unsuccessfully without an acceptable durable receipt.
    CommandFailed,
    /// The read exceeded its deadline.
    TimedOut,
    /// The command output exceeded the discovery limit.
    TooLarge,
    /// The caller no longer wants this observation.
    Cancelled,
    /// The provider record was malformed, unsupported or for another profile.
    Profile(profiles::Error),
    /// The provider reports that no profile registry exists on this machine.
    RegistryAbsent,
}

/// A discovery operation's executable and operator context, captured together.
/// Caller identity is preserved: this never converts an agent context into an
/// operator context or retries a refused command with fewer restrictions.
#[derive(Clone)]
pub struct Client {
    executable: PathBuf,
    directory: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

impl Client {
    /// Capture the explicit executable and current environment, using the same
    /// PATH that resolved the provider and the current working directory.
    /// No provider command or profile-data read runs here.
    pub fn new(executable: PathBuf, path_env: OsString) -> Result<Self, Error> {
        if !executable.is_absolute() {
            return Err(Error::Invalid);
        }
        let mut environment: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| key != "PATH")
            .collect();
        environment.push(("PATH".into(), path_env));
        Ok(Self {
            executable,
            directory: std::env::current_dir().map_err(|_| Error::Unavailable)?,
            environment,
        })
    }

    /// Every invocation uses the executable, effective environment and directory
    /// captured together, including delayed retries and read-only discovery.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .env_clear()
            .envs(self.environment.iter().cloned())
            .current_dir(&self.directory);
        command
    }

    /// List metadata only. Nonzero exits are errors, never an empty catalog.
    /// Run off the UI thread; cancellation stops the original child process.
    pub fn list(&self, live: &dyn Fn() -> bool) -> Result<Vec<Item>, Error> {
        let bytes = self.read(&["profiles", "list", "--json"], live, TIMEOUT)?;
        profiles::list(&bytes).map_err(Error::Profile)
    }

    /// Read one atomic record and reject a response for a different profile.
    /// A returned record still needs its authenticated connection handshake.
    pub fn show(&self, profile_id: &str, live: &dyn Fn() -> bool) -> Result<Profile, Error> {
        validate_id(profile_id)?;
        // End option parsing so even an opaque ID beginning with '-' is data.
        let bytes = self.read(
            &["profiles", "show", "--json", "--", profile_id],
            live,
            TIMEOUT,
        )?;
        let profile = Profile::parse(&bytes).map_err(Error::Profile)?;
        if profile.item().profile_id != profile_id {
            return Err(Error::Profile(profiles::Error::BindingMismatch));
        }
        Ok(profile)
    }

    /// Observe runtimes only in the retained profile context. Schema/flag errors
    /// are final; there is no retry through the unbound legacy catalog.
    pub fn runtimes(
        &self,
        identity: &profiles::Identity,
        live: &dyn Fn() -> bool,
    ) -> Result<profiles::runtime_catalog::Catalog, Error> {
        for value in [
            &identity.profile_id,
            &identity.installation_id,
            &identity.lifecycle_id,
        ] {
            validate_id(value)?;
        }
        // Joined option values preserve opaque IDs beginning with '-'.
        let profile = format!("--profile={}", identity.profile_id);
        let installation = format!("--expected-installation-id={}", identity.installation_id);
        let lifecycle = format!("--expected-lifecycle-id={}", identity.lifecycle_id);
        let bytes = self.read(
            &[
                "runtimes",
                "--json",
                "--check-setup",
                &profile,
                &installation,
                &lifecycle,
            ],
            live,
            TIMEOUT,
        )?;
        profiles::runtime_catalog::Catalog::parse(&bytes, identity).map_err(Error::Profile)
    }

    fn read(
        &self,
        arguments: &[&str],
        live: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<Vec<u8>, Error> {
        if !live() {
            return Err(Error::Cancelled);
        }
        let child = self
            .command()
            .args(arguments)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        let mut running = Running::new(child);
        let mut output = running.child.stdout.take().ok_or(Error::Unavailable)?;
        let flags = rustix::fs::fcntl_getfl(&output).map_err(|_| Error::Unavailable)?;
        rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::Unavailable)?;
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        let mut eof = false;
        let mut exited = false;
        loop {
            if !live() {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::TimedOut);
            }
            // Bounded nonblocking batches keep cancellation responsive even if
            // the writer never pauses. The kernel pipe supplies backpressure.
            for _ in 0..16 {
                if eof {
                    break;
                }
                let mut chunk = [0; 4096];
                match output.read(&mut chunk) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(read) => {
                        if read > OUTPUT_LIMIT - bytes.len() {
                            return Err(Error::TooLarge);
                        }
                        bytes.extend_from_slice(&chunk[..read]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(Error::Unavailable),
                }
            }
            if !exited {
                // NOWAIT keeps the leader unreaped until group cleanup, so its
                // PID cannot be recycled into an unrelated process group.
                let status = rustix::process::waitid(
                    rustix::process::WaitId::Pid(running.pid),
                    rustix::process::WaitIdOptions::EXITED
                        | rustix::process::WaitIdOptions::NOHANG
                        | rustix::process::WaitIdOptions::NOWAIT,
                )
                .map_err(|_| Error::Unavailable)?;
                if let Some(status) = status {
                    if status.exit_status() != Some(0) {
                        // A refusal written before exit is already in the pipe.
                        let mut chunk = [0; 4096];
                        while bytes.len() < OUTPUT_LIMIT {
                            match output.read(&mut chunk) {
                                Ok(read) if read > 0 => bytes.extend_from_slice(&chunk[..read]),
                                _ => break,
                            }
                        }
                        return Err(if registry_absent(&bytes) {
                            Error::RegistryAbsent
                        } else {
                            Error::CommandFailed
                        });
                    }
                    exited = true;
                    // A completed wrapper may leave descendants holding stdout.
                    // They belong to this command, never to a provider service.
                    running.terminate_group();
                }
            }
            if exited && eof {
                return if live() {
                    Ok(bytes)
                } else {
                    Err(Error::Cancelled)
                };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn registry_absent(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| {
        value["schema_version"] == 1 && value["error"]["code"] == "registry_absent"
    })
}

fn validate_id(profile_id: &str) -> Result<(), Error> {
    if profile_id.is_empty()
        || profile_id.len() > OUTPUT_LIMIT
        || !profile_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(Error::Invalid);
    }
    Ok(())
}

struct Running {
    child: Child,
    pid: rustix::process::Pid,
}
impl Running {
    fn new(child: Child) -> Self {
        let pid = rustix::process::Pid::from_child(&child);
        Self { child, pid }
    }
    fn terminate_group(&self) {
        if !self.pid.is_init() {
            let _ = rustix::process::kill_process_group(self.pid, rustix::process::Signal::KILL);
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.terminate_group();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests;
