//! Local control-plane execution retained by the original request and caller.
//! A timeout is an unknown outcome, never permission to allocate another ID.

use super::{Client, Error, OUTPUT_LIMIT};
use agent_interface_roster::profiles::{
    self,
    operations::{Prepared, Receipt},
};
use std::{
    ffi::OsString,
    os::unix::fs::FileExt,
    process::{Child, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(60);

/// One operation owns both its exact request and captured executable/environment.
/// Keep it after errors: only lookup or an explicit identical retry can resolve
/// uncertainty. No receipt is an authenticated content connection.
pub struct Operation {
    client: Client,
    prepared: Prepared,
}

impl Operation {
    /// Capture the caller context with an already prepared immutable request.
    /// This does not run a command or change a service.
    pub fn new(client: &Client, prepared: Prepared) -> Self {
        Self {
            client: client.clone(),
            prepared,
        }
    }

    /// Encode private request state. Store durably before submission; no
    /// environment values are included.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        super::recovery::checkpoint(
            super::recovery::Kind::Lifecycle,
            &self.prepared.checkpoint().map_err(Error::Profile)?,
        )
    }

    /// Restore under the current caller, however the app was launched this time.
    pub fn restore(client: &Client, bytes: &[u8]) -> Result<Self, Error> {
        let state = super::recovery::restore_checkpoint(super::recovery::Kind::Lifecycle, bytes)?;
        let prepared = Prepared::restore(&state).map_err(Error::Profile)?;
        Ok(Self::new(client, prepared))
    }

    /// Original action for recovery presentation; never inferred from selection.
    pub fn action(&self) -> profiles::operations::Action {
        self.prepared.action()
    }

    /// Original durable ID, unchanged across submissions and unknown outcomes.
    pub fn request_id(&self) -> &str {
        self.prepared.request_id()
    }

    /// Original exact request bytes for durable consumer storage and comparison.
    pub fn request_bytes(&self) -> &[u8] {
        self.prepared.bytes()
    }

    /// A previously validated terminal result, retained through private recovery.
    pub fn terminal(&self) -> Option<&Receipt> {
        self.prepared.terminal()
    }

    /// Original or provider-reserved tuple, never content connection authority.
    pub fn identity(&self) -> Option<&profiles::Identity> {
        self.prepared.identity()
    }

    /// Submit or explicitly retry the exact original request off the UI thread.
    /// Cancellation only stops waiting for this invocation; it does not claim
    /// that the provider rolled back or cancelled its durable operation.
    pub fn apply(&mut self, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.apply_with_timeout(live, TIMEOUT)
    }

    fn apply_with_timeout(
        &mut self,
        live: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<Receipt, Error> {
        if !live() {
            return Err(Error::Cancelled);
        }
        let input = super::invocation_file::InvocationFile::new(self.prepared.bytes())?;
        let args: Vec<OsString> = vec![
            "profiles".into(),
            "apply".into(),
            "--file".into(),
            input.path().into(),
            "--json".into(),
        ];
        let (status, bytes) = self.run(&args, live, timeout)?;
        self.observe(status, &bytes)
    }

    /// Read the original durable receipt without reissuing the action. Unknown
    /// request replies remain errors and preserve all retained request state.
    pub fn refresh(&mut self, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        let args: Vec<OsString> = vec![
            "profiles".into(),
            "operation".into(),
            "--json".into(),
            "--".into(),
            self.request_id().into(),
        ];
        let (status, bytes) = self.run(&args, live, TIMEOUT)?;
        self.observe(status, &bytes)
    }

    fn observe(&mut self, status: ExitStatus, bytes: &[u8]) -> Result<Receipt, Error> {
        // A durable failure receipt may accompany a nonzero exit. A signal or
        // unsuccessful process cannot establish successful/pending completion.
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| Error::Profile(profiles::Error::Invalid))?;
        if status.code().is_none()
            || (!status.success()
                && value.get("status").and_then(serde_json::Value::as_str) != Some("failed"))
        {
            return Err(Error::CommandFailed);
        }
        self.prepared.observe(bytes).map_err(Error::Profile)
    }

    fn run(
        &self,
        args: &[OsString],
        live: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<(ExitStatus, Vec<u8>), Error> {
        if !live() {
            return Err(Error::Cancelled);
        }
        // A service may inherit this descriptor. A regular anonymous file stays
        // writable after the app closes its copy, unlike a pipe (SIGPIPE/EPIPE).
        // The provider remains responsible for its service logging destination.
        let output = super::invocation_file::InvocationFile::new(&[])?.into_file();
        let child = self
            .client
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(|_| Error::Unavailable)?)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        let mut running = Invocation(child);
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        loop {
            if !live() {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::TimedOut);
            }
            drain(&output, &mut bytes)?;
            if let Some(status) = running.0.try_wait().map_err(|_| Error::Unavailable)? {
                // The provider may start a long-lived service inheriting stdout.
                // Drain available bytes but do not wait for descendants or kill
                // their process group. The direct command owns this receipt.
                drain(&output, &mut bytes)?;
                return if live() {
                    Ok((status, bytes))
                } else {
                    Err(Error::Cancelled)
                };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn drain(output: &std::fs::File, bytes: &mut Vec<u8>) -> Result<(), Error> {
    let length = output.metadata().map_err(|_| Error::Unavailable)?.len();
    if length > OUTPUT_LIMIT as u64 {
        return Err(Error::TooLarge);
    }
    // Read only the observed bounded length, using a private offset so reads
    // never move the shared descriptor's write position in the child.
    while (bytes.len() as u64) < length {
        let mut chunk = [0; 4096];
        let wanted = chunk.len().min(length as usize - bytes.len());
        match output.read_at(&mut chunk[..wanted], bytes.len() as u64) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if n > OUTPUT_LIMIT - bytes.len() {
                    return Err(Error::TooLarge);
                }
                bytes.extend_from_slice(&chunk[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Unavailable),
        }
    }
    Ok(())
}

struct Invocation(Child);
impl Drop for Invocation {
    fn drop(&mut self) {
        // Only the original direct child is ours to terminate. Service children
        // and durable provider state must survive unknown outcomes.
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests;
