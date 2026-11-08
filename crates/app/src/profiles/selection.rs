//! A cancellable selection handoff. Registry metadata alone never activates a
//! workspace; only an accepted authenticated connection can be handed to it.

use super::{Client, Error, validate_id};
use agent_interface_roster::profiles::{Connection, Item};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// One verified selection, ready for an explicit workspace binding decision.
/// Existing panes keep their own connections; this value does not retarget them.
pub struct Selected {
    item: Item,
    connection: Connection,
}
impl Selected {
    /// Metadata from the same atomic registry show that supplied the binding.
    pub fn item(&self) -> &Item {
        &self.item
    }
    /// Authenticated endpoint, credentials and full lifecycle identity.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

struct Pending {
    profile_id: String,
    live: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}

/// Per-picker attempt ownership. Use separate instances for separate workspaces.
/// Starting a new attempt or dropping this owner invalidates older completions.
#[derive(Default)]
pub struct Selection {
    pending: Option<Pending>,
}
impl Selection {
    /// Capture a choice and the original operator discovery context. No I/O.
    /// A repeated choice has a new ticket, including the same profile ID.
    pub fn begin(&mut self, client: &Client, profile_id: &str) -> Result<Attempt, Error> {
        validate_id(profile_id)?;
        let live = Arc::new(AtomicBool::new(true));
        self.pending = Some(Pending {
            profile_id: profile_id.into(),
            live: live.clone(),
        });
        Ok(Attempt {
            client: client.clone(),
            profile_id: profile_id.into(),
            live,
        })
    }
    /// The requested registry ID while verification is pending, never authority.
    pub fn pending(&self) -> Option<&str> {
        self.pending
            .as_ref()
            .map(|pending| pending.profile_id.as_str())
    }
    /// Cancel the choice without stopping services or changing any workspace.
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    /// Accept exactly this owner's latest attempt, once. Stale successes and
    /// failures are ignored. A failure yields no replacement connection.
    pub fn accept(&mut self, completion: Completion) -> Option<Result<Selected, Error>> {
        let pending = self.pending.as_ref()?;
        if !Arc::ptr_eq(&pending.live, &completion.live)
            || !pending.live.load(Ordering::Acquire)
            || pending.profile_id != completion.profile_id
        {
            return None;
        }
        self.pending = None;
        Some(completion.result)
    }
}

/// One off-thread registry lookup followed by the original identity handshake.
/// It owns the captured executable/environment and never retries through Default.
pub struct Attempt {
    client: Client,
    profile_id: String,
    live: Arc<AtomicBool>,
}
impl Attempt {
    /// Run on an I/O worker. Discovery cancellation stops its command; a started
    /// local handshake has the transport's bounded deadline and is discarded if
    /// cancelled before completion. No service or account sign-in commands are run.
    pub fn run(self) -> Completion {
        let live = || self.live.load(Ordering::Acquire);
        let result = self
            .client
            .show(&self.profile_id, &live)
            .and_then(|profile| {
                if !live() {
                    return Err(Error::Cancelled);
                }
                let connection = profile.connect().map_err(Error::Profile)?;
                if !live() {
                    return Err(Error::Cancelled);
                }
                Ok(Selected {
                    item: profile.item().clone(),
                    connection,
                })
            });
        Completion {
            profile_id: self.profile_id,
            live: self.live,
            result,
        }
    }
}

/// Opaque completion carrying its original ticket; only `Selection::accept`
/// exposes a verified result. It cannot be relabelled for a later choice.
pub struct Completion {
    profile_id: String,
    live: Arc<AtomicBool>,
    result: Result<Selected, Error>,
}

#[cfg(test)]
mod tests;
