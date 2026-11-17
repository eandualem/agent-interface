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
/// Longer than the provider's own 60-second limit, so its receipt arrives.
const APPLY_TIMEOUT: Duration = Duration::from_secs(90);

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

    /// Whether the original create request asks for this computer's Git identity.
    pub fn copies_git_identity(&self) -> bool {
        self.prepared.copies_git_identity()
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
        self.apply_with_timeout(live, APPLY_TIMEOUT)
    }

    /// Send the exact original request again after an unknown outcome. Only
    /// the earlier provider's receipt, or the Rust service's profile, settles
    /// it, or a refusal, which the service gives only when nothing was made.
    pub fn retry(&mut self, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.submit(live, APPLY_TIMEOUT, true)
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
        if self.served_create(live)? {
            return self.create_served(live, timeout, repeat);
        }
        let input = super::invocation_file::InvocationFile::new(self.prepared.bytes())?;
        let args: Vec<OsString> = vec![
            "profiles".into(),
            "apply".into(),
            "--file".into(),
            input.path().into(),
            "--json".into(),
        ];
        // A start or stop that loses its command keeps its reservation in the
        // provider, which then refuses every later one. Leaving a screen or quitting
        // never stops it: the command finishes and releases the reservation itself.
        // It may read `--file` only after this call returns, so the file goes with it.
        let (status, bytes) = self.run(&args, live, timeout, Some(input))?;
        self.observe(status, &bytes)
    }

    /// Read the original durable receipt without reissuing the action. Unknown
    /// request replies remain errors and preserve all retained request state.
    pub fn refresh(&mut self, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        // The Rust service keeps no operation to read. It answers the same
        // request ID with its first result, so asking again reads that.
        if self.served_create(live)? {
            return self.create_served(live, TIMEOUT, true);
        }
        let args: Vec<OsString> = vec![
            "profiles".into(),
            "operation".into(),
            "--json".into(),
            "--".into(),
            self.request_id().into(),
        ];
        let (status, bytes) = self.run(&args, live, TIMEOUT, None)?;
        self.observe(status, &bytes)
    }

    /// Whether this is a create request for the Rust service, which makes a
    /// profile with `profiles create` instead of `profiles apply`.
    /// A list that fails says nothing: the earlier provider has none until
    /// its first profile is made, so `profiles apply` is tried as before.
    fn served_create(&self, live: &dyn Fn() -> bool) -> Result<bool, Error> {
        if self.prepared.action() != profiles::operations::Action::Create {
            return Ok(false);
        }
        match self.client.served(live) {
            Err(Error::Cancelled) => Err(Error::Cancelled),
            result => Ok(result.unwrap_or(false)),
        }
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
        let (status, bytes) = self.run(&args, live, timeout, None)?;
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
    /// made none. A list that fails leaves it unknown.
    fn unnamed(&self, name: &str, live: &dyn Fn() -> bool) -> Result<bool, Error> {
        match self.client.list(live) {
            Ok(items) => Ok(items.iter().all(|item| item.display_name != name)),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(_) => Ok(false),
        }
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
        keep_running: Option<super::invocation_file::InvocationFile>,
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
        let mut running = Invocation {
            child: Some(child),
            keep_running,
        };
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
    /// Some to leave the child running, holding its request file until it exits.
    keep_running: Option<super::invocation_file::InvocationFile>,
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
        if let Some(input) = self.keep_running.take() {
            // It writes to an anonymous file, so it outlives this process safely.
            std::thread::spawn(move || {
                let _ = child.wait();
                drop(input);
            });
            return;
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests;
