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
/// How long a create may take before its outcome is unknown.
const CREATE_TIMEOUT: Duration = Duration::from_secs(90);

/// One operation owns both its exact request and captured executable/environment.
/// Keep it after errors: only asking again under the same ID can resolve
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

    /// The name the owner entered in this create request.
    pub fn display_name(&self) -> Option<String> {
        self.prepared.display_name()
    }

    /// Original durable ID, unchanged across submissions and unknown outcomes.
    pub fn request_id(&self) -> &str {
        self.prepared.request_id()
    }

    /// Original exact request bytes for durable consumer storage and comparison.
    pub fn request_bytes(&self) -> &[u8] {
        self.prepared.bytes()
    }

    /// Whether the original create request asks for this computer's Git identity.
    pub fn copies_git_identity(&self) -> bool {
        self.prepared.copies_git_identity()
    }

    /// A previously validated terminal result, retained through private recovery.
    pub fn terminal(&self) -> Option<&Receipt> {
        self.prepared.terminal()
    }

    /// The new profile's identity once made, never content connection authority.
    pub fn identity(&self) -> Option<&profiles::Identity> {
        self.prepared.identity()
    }

    /// Submit the exact original request off the UI thread. Cancellation
    /// only stops waiting for this invocation; it does not claim that the
    /// service undid anything.
    pub fn apply(&mut self, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.apply_with_timeout(live, CREATE_TIMEOUT)
    }

    /// Send the exact original request again after an unknown outcome. Only
    /// the profile the service made settles it, or a refusal, which the
    /// service gives only when nothing was made.
    pub fn retry(&mut self, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.submit(live, CREATE_TIMEOUT, true)
    }

    fn apply_with_timeout(
        &mut self,
        live: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<Receipt, Error> {
        self.submit(live, timeout, false)
    }

    fn submit(
        &mut self,
        live: &dyn Fn() -> bool,
        timeout: Duration,
        repeat: bool,
    ) -> Result<Receipt, Error> {
        if !live() {
            return Err(Error::Cancelled);
        }
        self.create_served(live, timeout, repeat)
    }

    /// Make the profile with the Rust service under this request's own ID.
    /// It serves the profile at once, so there is nothing to start after.
    /// A `repeat` follows an unknown outcome, so only what it shows of the
    /// first send settles it.
    fn create_served(
        &mut self,
        live: &dyn Fn() -> bool,
        timeout: Duration,
        repeat: bool,
    ) -> Result<Receipt, Error> {
        let name = self
            .prepared
            .display_name()
            .ok_or(Error::Profile(profiles::Error::Invalid))?;
        let args: Vec<OsString> = vec![
            "profiles".into(),
            "create".into(),
            "--starter".into(),
            "core-v1".into(),
            "--request-id".into(),
            self.request_id().into(),
            "--json".into(),
            // A name beginning with '-' is still the name.
            "--".into(),
            name.as_str().into(),
        ];
        let (status, bytes) = self.run(&args, live, timeout)?;
        // On a first send, 1 is a refusal and 3 a service it could not reach:
        // nothing was made. 2 is a request it could not read, refused before
        // sending, then and on any repeat. A repeat can be refused although
        // the first send made the profile, such as when its ID is too old to
        // accept, so it is settled only when no profile has the name. Any
        // other failure, 4 included, leaves the outcome unknown, and asking
        // again under the same ID reads it.
        let refused = match status.code() {
            Some(0) => {
                let profile = profiles::Profile::parse(&bytes).map_err(Error::Profile)?;
                return self
                    .prepared
                    .observe_created(&profile)
                    .map_err(Error::Profile);
            }
            Some(1) if !repeat || self.unnamed(&name, live)? => "refused",
            Some(2) => "refused",
            Some(3) if !repeat => "unreachable",
            _ => return Err(Error::CommandFailed),
        };
        self.prepared
            .observe_refused(refused)
            .map_err(Error::Profile)
    }

    /// Whether the machine lists no profile named `name`, so a refused repeat
    /// made none. The service's own entries are read: one without a name, or
    /// in a state this app does not know, may be the one made, and so may a
    /// list that fails or cannot be read.
    fn unnamed(&self, name: &str, live: &dyn Fn() -> bool) -> Result<bool, Error> {
        use serde_json::Value;
        let bytes = match self
            .client
            .read(&["profiles", "list", "--json"], live, TIMEOUT)
        {
            Ok(bytes) => bytes,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => return Ok(false),
        };
        let listed: Option<Value> = serde_json::from_slice(&bytes).ok();
        let Some(entries) = listed.as_ref().and_then(|list| list["profiles"].as_array()) else {
            return Ok(false);
        };
        Ok(entries.iter().all(|entry| {
            matches!(entry["state"].as_str(), Some("serving" | "failed"))
                && entry["display_name"]
                    .as_str()
                    .is_some_and(|listed| listed != name)
        }))
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
        let mut running = Invocation { child: Some(child) };
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
            if let Some(status) = running.try_wait().map_err(|_| Error::Unavailable)? {
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

struct Invocation {
    child: Option<Child>,
}
impl Invocation {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.as_mut().map_or(Ok(None), Child::try_wait)
    }
}
impl Drop for Invocation {
    fn drop(&mut self) {
        // Only the original direct child is ours to terminate. Service children
        // and durable provider state must survive unknown outcomes.
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests;
