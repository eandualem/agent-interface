//! Retained-content polling pinned to one verified connection for its lifetime.

use std::{
    io,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use agent_interface_roster::{profiles::Connection, views::content};

/// A content observation and the selection generation that requested it.
#[derive(Clone, Debug)]
pub struct BoundContentUpdate {
    /// Changes on every selection/visibility change, even for repeated requests.
    pub generation: u64,
    /// Original full connection identity, request and result.
    pub read: content::Read,
}

struct State {
    request: content::Request,
    generation: u64,
    active: bool,
    stopped: bool,
    update: Option<BoundContentUpdate>,
}

impl State {
    fn wants(&self, generation: u64) -> bool {
        !self.stopped && self.active && self.generation == generation
    }
}

/// A private worker and pending-result slot; it cannot be rebound to a profile.
pub struct BoundContentSource {
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl BoundContentSource {
    /// Read immediately, then `every` after each read, except explicit Documents.
    /// `wake` belongs to the original owner and runs after publishing an update.
    pub fn start(
        connection: Connection,
        request: content::Request,
        every: Duration,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let state = Arc::new((
            Mutex::new(State {
                request,
                generation: 0,
                active: true,
                stopped: false,
                update: None,
            }),
            Condvar::new(),
        ));
        let shared = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("bound content polling".into())
            .spawn(move || {
                let (lock, changed) = &*shared;
                loop {
                    let state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let state = changed
                        .wait_while(state, |state| !state.stopped && !state.active)
                        .unwrap_or_else(|e| e.into_inner());
                    if state.stopped {
                        break;
                    }
                    let (request, generation) = (state.request.clone(), state.generation);
                    drop(state);
                    let read = content::read(&connection, &request, &|| {
                        lock.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .wants(generation)
                    });
                    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.wants(generation) {
                        continue;
                    }
                    state.update = Some(BoundContentUpdate { generation, read });
                    drop(state);
                    wake();
                    let state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    // Predicate waits preserve changes made during the read or
                    // wake callback, and ignore spurious condition-variable wakes.
                    if request.explicit() {
                        drop(
                            changed
                                .wait_while(state, |state| state.wants(generation))
                                .unwrap_or_else(|e| e.into_inner()),
                        );
                    } else {
                        drop(
                            changed
                                .wait_timeout_while(state, every, |state| state.wants(generation))
                                .unwrap_or_else(|e| e.into_inner()),
                        );
                    }
                }
            })?;
        Ok(Self {
            state,
            worker: Some(worker),
        })
    }

    /// Select or refresh content on this connection, returning its new generation.
    /// Clears even a completed pending result for the previous selection.
    pub fn set_request(&self, request: content::Request) -> u64 {
        let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
        state.request = request;
        state.generation += 1;
        state.update = None;
        self.state.1.notify_one();
        state.generation
    }

    /// Pause hidden panes; resume with a fresh read of their existing selection.
    pub fn set_active(&self, active: bool) -> u64 {
        let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.active != active {
            state.active = active;
            state.generation += 1;
            state.update = None;
            self.state.1.notify_one();
        }
        state.generation
    }

    /// Take the latest result for the current selection. Deferred consumers must
    /// still check the read's identity and generation against their own owner.
    pub fn take_update(&self) -> Option<BoundContentUpdate> {
        self.state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update
            .take()
    }
}

impl Drop for BoundContentSource {
    fn drop(&mut self) {
        {
            let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            state.update = None;
            self.state.1.notify_one();
        }
        // Finish only the bounded in-flight request, not the remaining chain.
        // UI owners must dispose of this handle off the UI thread.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
