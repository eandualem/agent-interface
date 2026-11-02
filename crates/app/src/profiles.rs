//! Read-only local operator discovery. This does not select a profile, connect
//! to its content endpoint, adopt an installation or change service state.

use agent_interface_roster::profiles::{self, Item, Profile};
use std::{
    ffi::OsString,
    os::unix::fs::FileExt,
    path::PathBuf,
    process::{Command, Stdio},
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
    /// Discovery exited unsuccessfully, including an operator-role refusal.
    CommandFailed,
    /// The read exceeded its deadline.
    TimedOut,
    /// The command output exceeded the discovery limit.
    TooLarge,
    /// The caller no longer wants this observation.
    Cancelled,
    /// The provider record was malformed, unsupported or for another profile.
    Profile(profiles::Error),
}

/// A discovery operation's executable and operator context, captured together.
/// Caller identity is preserved: this never converts an agent context into an
/// operator context or retries a refused command with fewer restrictions.
#[derive(Clone)]
pub struct Client {
    executable: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

impl Client {
    /// Capture the explicit executable and current environment, using the same
    /// PATH that resolved the provider. No command or filesystem read runs here.
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
            environment,
        })
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
        if profile_id.is_empty()
            || profile_id.len() > OUTPUT_LIMIT
            || !profile_id.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(Error::Invalid);
        }
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

    fn read(
        &self,
        arguments: &[&str],
        live: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<Vec<u8>, Error> {
        if !live() {
            return Err(Error::Cancelled);
        }
        // An inherited output pipe must not keep this read alive after the CLI
        // exits. An anonymous file also avoids exposing a temporary output path.
        let output = tempfile::tempfile().map_err(|_| Error::Unavailable)?;
        let mut child = Command::new(&self.executable)
            .env_clear()
            .envs(self.environment.iter().cloned())
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(|_| Error::Unavailable)?)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        let deadline = Instant::now() + timeout;
        let result = (|| {
            loop {
                if !live() {
                    return Err(Error::Cancelled);
                }
                if output.metadata().map_err(|_| Error::Unavailable)?.len() > OUTPUT_LIMIT as u64 {
                    return Err(Error::TooLarge);
                }
                match child.try_wait().map_err(|_| Error::Unavailable)? {
                    Some(exit) if exit.success() => break,
                    Some(_) => return Err(Error::CommandFailed),
                    None if Instant::now() >= deadline => return Err(Error::TimedOut),
                    None => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            let mut bytes = vec![0; OUTPUT_LIMIT + 1];
            let mut length = 0;
            while length < bytes.len() {
                match output.read_at(&mut bytes[length..], length as u64) {
                    Ok(0) => break,
                    Ok(read) => length += read,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Err(Error::Unavailable),
                }
            }
            if !live() {
                return Err(Error::Cancelled);
            }
            if length > OUTPUT_LIMIT {
                return Err(Error::TooLarge);
            }
            bytes.truncate(length);
            Ok(bytes)
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }
}

#[cfg(test)]
mod tests;
