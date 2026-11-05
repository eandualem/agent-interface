//! Explicit schema-2 setup execution in the captured operator context.

use super::{Client, Error, OUTPUT_LIMIT};
use agent_interface_roster::profiles::setup::{Action, Event, Prepared, Stream};
use std::{
    fs::File,
    os::unix::fs::FileExt,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

/// One original setup request, including its exact cancellation body. Retain
/// across uncertain outcomes; cancellation never allocates a replacement ID.
pub struct Setup {
    client: Client,
    prepared: Prepared,
}

impl Setup {
    /// Capture the client and original choices without executing anything.
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
            super::recovery::Kind::Setup,
            &self.prepared.checkpoint().map_err(Error::Profile)?,
        )
    }

    /// Restore under the current caller, however the app was launched this time.
    pub fn restore(client: &Client, bytes: &[u8]) -> Result<Self, Error> {
        let state = super::recovery::restore_checkpoint(super::recovery::Kind::Setup, bytes)?;
        let prepared = Prepared::restore(&state).map_err(Error::Profile)?;
        Ok(Self::new(client, prepared))
    }

    /// Original setup operation ID, unchanged for cancellation and recovery.
    pub fn request_id(&self) -> &str {
        self.prepared.request_id()
    }

    /// The agent this request adds.
    pub fn agent_name(&self) -> &str {
        self.prepared.agent_name()
    }

    /// Original expected tuple; readiness never substitutes for a handshake.
    pub fn identity(&self) -> &agent_interface_roster::profiles::Identity {
        self.prepared.identity()
    }

    /// Encode an outcome returned after this executor verified process exit.
    /// Store alongside the original request capsule.
    pub fn outcome_checkpoint(&self, event: &Event) -> Result<Vec<u8>, Error> {
        self.prepared
            .outcome_checkpoint(event)
            .map_err(Error::Profile)
    }

    /// Validate a stored outcome against this original request and action. The
    /// caller must retain durable evidence that the executor accepted its exit.
    pub fn restore_outcome(&self, action: Action, bytes: &[u8]) -> Result<Event, Error> {
        self.prepared
            .restore_outcome(action, bytes)
            .map_err(Error::Profile)
    }

    /// Exact original or cancellation bytes for durable consumer storage.
    pub fn bytes(&self, action: Action) -> &[u8] {
        self.prepared.bytes(action)
    }

    /// Run one explicit submission or exact retry. Only validated progress steps
    /// are emitted while running; terminal authority is returned after a matching
    /// process exit. Stopping observation leaves an unknown outcome, not a
    /// successful cancellation. Run off the UI thread.
    pub fn run(
        &self,
        action: Action,
        live: &dyn Fn() -> bool,
        emit: impl FnMut(Event),
    ) -> Result<Event, Error> {
        self.run_with_timeout(action, live, emit, Duration::from_secs(300))
    }

    fn run_with_timeout(
        &self,
        action: Action,
        live: &dyn Fn() -> bool,
        mut emit: impl FnMut(Event),
        timeout: Duration,
    ) -> Result<Event, Error> {
        if !live() {
            return Err(Error::Cancelled);
        }
        let input = super::invocation_file::InvocationFile::new(self.bytes(action))?;
        // A regular anonymous sink does not SIGPIPE a service/agent that
        // inherits stdout after this invocation finishes. Provider owns logging.
        let output = super::invocation_file::InvocationFile::new(&[])?.into_file();
        let child = self
            .client
            .command()
            .args(["setup", "--file"])
            .arg(input.path())
            .arg("--jsonl")
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(|_| Error::Unavailable)?)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        let mut running = Invocation(child);
        let mut stream = self.prepared.stream(action);
        let mut offset = 0;
        let deadline = Instant::now() + timeout;
        loop {
            if !live() {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::TimedOut);
            }
            drain(&output, &mut offset, &mut stream, &mut emit)?;
            if let Some(status) = running.0.try_wait().map_err(|_| Error::Unavailable)? {
                drain(&output, &mut offset, &mut stream, &mut emit)?;
                if !live() {
                    return Err(Error::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(Error::TimedOut);
                }
                let event = stream.finish().map_err(Error::Profile)?;
                let consistent = match event.status.as_str() {
                    "completed" => status.code() == Some(0),
                    "action_required" => status.code() == Some(3),
                    "failed" => matches!(status.code(), Some(1 | 2)),
                    _ => false,
                };
                return if consistent {
                    Ok(event)
                } else {
                    Err(Error::CommandFailed)
                };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn drain(
    file: &File,
    offset: &mut u64,
    stream: &mut Stream,
    emit: &mut impl FnMut(Event),
) -> Result<(), Error> {
    let length = file.metadata().map_err(|_| Error::Unavailable)?.len();
    if length > OUTPUT_LIMIT as u64 {
        return Err(Error::TooLarge);
    }
    while *offset < length {
        let mut chunk = [0; 8192];
        let wanted = chunk.len().min((length - *offset) as usize);
        match file.read_at(&mut chunk[..wanted], *offset) {
            Ok(0) => break,
            Ok(n) => {
                *offset += n as u64;
                for event in stream.push(&chunk[..n]).map_err(Error::Profile)? {
                    if event.step.is_some() {
                        emit(event);
                    }
                }
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
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests;
