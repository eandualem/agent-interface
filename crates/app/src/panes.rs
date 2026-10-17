//! The panes of one control-mode connection, each kept as terminal state on a
//! terminal worker thread
//! (`docs/decisions/0007-threads-ownership-and-event-ordering.md`).
//!
//! A reader thread reads the tmux client's output and decodes it. Each pane's
//! output goes, in order, to the terminal worker, which owns every
//! [`Terminal`] and copies the panes that changed into owned [`Screen`]s. At
//! most one update waits for the user interface. While it waits, the worker
//! keeps applying output. A pane that changes and is not in the waiting update
//! joins it at once; the next update holds every other pane that changed
//! meanwhile, so the user interface copies at most one update per frame.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use tmux_workspaces_term::{Screen, Terminal};
use tmux_workspaces_tmux::command;
use tmux_workspaces_tmux::control::{Decoder, Message, PaneId};

/// The most pane output the reader queues for the worker before it stops
/// reading, so a slow worker holds back tmux instead of growing a queue.
const QUEUE_LIMIT: usize = 4 * 1024 * 1024;

/// How much of the client's output the reader reads at a time.
const READ_SIZE: usize = 64 * 1024;

/// A pane's place in its tmux window, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneLayout {
    /// The pane.
    pub id: PaneId,
    /// The column of its left edge.
    pub left: u16,
    /// The row of its top edge.
    pub top: u16,
    /// Its width.
    pub cols: u16,
    /// Its height.
    pub rows: u16,
}

/// The screens of the panes that changed since the previous update.
pub type Update = Vec<(PaneId, Screen)>;

/// The panes of one control-mode client, kept up to date in the background.
///
/// Dropping it detaches the client, waits for it to exit and stops both
/// threads. The tmux session keeps running.
pub struct Panes {
    shared: Arc<Shared>,
    stdin: Mutex<Option<ChildStdin>>,
    client: Child,
    threads: Vec<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// Pane output the worker has not applied yet, in the order tmux sent it.
    queue: VecDeque<(PaneId, Vec<u8>)>,
    /// The bytes in `queue`.
    queued: usize,
    /// The update waiting for the user interface.
    update: Option<Update>,
    /// Screens the user interface no longer needs, for the worker to reuse.
    spare: Vec<Screen>,
    /// Whether the client has exited: the reader has queued everything it
    /// will.
    closed: bool,
    /// Whether the panes are being dropped, so pending work is abandoned.
    cancelled: bool,
    /// Whether the worker has stopped, after handing over every change, or
    /// because the panes are being dropped.
    finished: bool,
}

impl Panes {
    /// Starts keeping the panes in `layout` up to date from `client`, a
    /// control-mode client (`tmux -C`) with piped standard input and output,
    /// already attached to their session. Each pane keeps about `scrollback`
    /// lines of history.
    ///
    /// `wake` is called on the worker thread whenever an update is ready, and
    /// once more when the worker has finished.
    pub fn start(
        mut client: Child,
        layout: &[PaneLayout],
        scrollback: usize,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let stdin = client.stdin.take();
        let stdout = client
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("the client's standard output is not piped"))?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let layout = layout.to_vec();
        let threads = vec![
            spawn("tmux reader", {
                let shared = Arc::clone(&shared);
                move || read(&shared, stdout)
            })?,
            spawn("terminal worker", {
                let shared = Arc::clone(&shared);
                move || work(&shared, &layout, scrollback, &wake)
            })?,
        ];
        Ok(Self {
            shared,
            stdin: Mutex::new(stdin),
            client,
            threads,
        })
    }

    /// Takes the waiting update, if there is one.
    pub fn take_update(&self) -> Option<Update> {
        let update = self.shared.lock().update.take();
        if update.is_some() {
            // The worker may have changes waiting for this slot.
            self.shared.changed.notify_all();
        }
        update
    }

    /// Gives back screens the user interface has replaced, so the worker can
    /// reuse their allocations.
    pub fn recycle(&self, screens: impl IntoIterator<Item = Screen>) {
        self.shared.lock().spare.extend(screens);
    }

    /// Sends `bytes` to `pane` as literal input.
    pub fn send_input(&self, pane: PaneId, bytes: &[u8]) -> io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("the client is detached"))?;
        stdin.write_all(command::send_bytes(pane, bytes).as_bytes())?;
        stdin.flush()
    }

    /// Returns whether the client has exited, the worker has handed over
    /// every change its output made, and the last update has been taken.
    pub fn is_finished(&self) -> bool {
        let state = self.shared.lock();
        state.finished && state.update.is_none()
    }
}

impl Drop for Panes {
    fn drop(&mut self) {
        // Closing standard input detaches the client, which then exits and
        // ends the reader with end of file.
        drop(self.stdin.lock().unwrap_or_else(|e| e.into_inner()).take());
        self.shared.lock().cancelled = true;
        self.shared.changed.notify_all();
        let _ = self.client.wait();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking thread leaves the state consistent: every change is a
        // single assignment or queue operation.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed.wait(state).unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.to_owned()).spawn(body)
}

/// Reads and decodes the client's output, queueing each pane's output for the
/// worker, until the client exits.
fn read(shared: &Shared, mut stdout: ChildStdout) {
    let mut decoder = Decoder::new();
    let mut buffer = vec![0; READ_SIZE];
    loop {
        let read = match stdout.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let mut batch: Vec<(PaneId, Vec<u8>)> = Vec::new();
        let mut exited = false;
        let decoded = decoder.decode(&buffer[..read], |message| match message {
            Message::Output { pane, data } => match batch.last_mut() {
                Some((last, bytes)) if *last == pane => bytes.extend_from_slice(data),
                _ => batch.push((pane, data.to_vec())),
            },
            Message::Notification(line) if line.starts_with(b"%exit") => exited = true,
            _ => {}
        });
        if decoded.is_err() {
            break;
        }
        let size: usize = batch.iter().map(|(_, bytes)| bytes.len()).sum();
        let mut state = shared.lock();
        while state.queued > QUEUE_LIMIT && !state.cancelled {
            state = shared.wait(state);
        }
        if state.cancelled {
            break;
        }
        state.queued += size;
        state.queue.extend(batch);
        drop(state);
        shared.changed.notify_all();
        if exited {
            break;
        }
    }
    shared.close();
}

/// Applies queued output to each pane's terminal and publishes updates, until
/// the client has exited and the queue is empty.
fn work(shared: &Shared, layout: &[PaneLayout], scrollback: usize, wake: &dyn Fn()) {
    // Engine handles are not `Send`, so they are made on this thread.
    let terminals: Result<Vec<_>, _> = layout
        .iter()
        .map(|pane| Terminal::new(pane.cols, pane.rows, scrollback).map(|t| (pane.id, t)))
        .collect();
    let Ok(mut terminals) = terminals else {
        shared.lock().finished = true;
        shared.changed.notify_all();
        wake();
        return;
    };
    let mut changed: Vec<PaneId> = Vec::new();
    let mut state = shared.lock();
    loop {
        if !changed.is_empty() && state.update.is_none() && !state.cancelled {
            let mut spare = std::mem::take(&mut state.spare);
            drop(state);
            let mut update = Vec::with_capacity(changed.len());
            for pane in changed.drain(..) {
                let Some((_, terminal)) = terminals.iter_mut().find(|(id, _)| *id == pane) else {
                    continue;
                };
                let mut screen = spare.pop().unwrap_or_default();
                if terminal.read_screen(&mut screen).is_ok() {
                    update.push((pane, screen));
                }
            }
            state = shared.lock();
            state.spare.extend(spare);
            state.update = Some(update);
            drop(state);
            wake();
            state = shared.lock();
        } else if let Some(joining) = joining(&state, &changed) {
            // A pane that changed after the waiting update was made, and is
            // not in it, joins it now, so that a quiet pane's change, such as
            // a key's echo, does not wait a frame behind a busy neighbour.
            let mut spare = std::mem::take(&mut state.spare);
            drop(state);
            let mut copies = Vec::with_capacity(joining.len());
            for &pane in &joining {
                let Some((_, terminal)) = terminals.iter_mut().find(|(id, _)| *id == pane) else {
                    continue;
                };
                let mut screen = spare.pop().unwrap_or_default();
                if terminal.read_screen(&mut screen).is_ok() {
                    copies.push((pane, screen));
                }
            }
            state = shared.lock();
            if let Some(waiting) = state.update.as_mut() {
                changed.retain(|pane| !joining.contains(pane));
                waiting.extend(copies);
            } else {
                // Taken meanwhile: the panes stay changed, and the next
                // update carries them.
                spare.extend(copies.into_iter().map(|(_, screen)| screen));
            }
            state.spare.extend(spare);
        } else if let Some((pane, bytes)) = state.queue.pop_front() {
            state.queued -= bytes.len();
            drop(state);
            // The reader may be waiting for room in the queue.
            shared.changed.notify_all();
            if let Some((_, terminal)) = terminals.iter_mut().find(|(id, _)| *id == pane) {
                terminal.write(&bytes);
                if !changed.contains(&pane) {
                    changed.push(pane);
                }
            }
            state = shared.lock();
        } else if state.cancelled || (state.closed && changed.is_empty()) {
            // Once the client has exited, the last changes still wait for the
            // user interface to take the update before them.
            state.finished = true;
            drop(state);
            wake();
            return;
        } else {
            state = shared.wait(state);
        }
    }
}

/// Returns the panes in `changed` that the waiting update does not hold, or
/// `None` when there are none, no update waits or the panes are being dropped.
fn joining(state: &State, changed: &[PaneId]) -> Option<Vec<PaneId>> {
    let waiting = state.update.as_ref().filter(|_| !state.cancelled)?;
    let absent: Vec<PaneId> = changed
        .iter()
        .copied()
        .filter(|pane| !waiting.iter().any(|(id, _)| id == pane))
        .collect();
    (!absent.is_empty()).then_some(absent)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use tmux_workspaces_test_support::TmuxServer;

    use super::*;

    /// Waits until the pane's screen shows `text` on its first row.
    fn wait_for_row(panes: &Panes, woken: &mpsc::Receiver<()>, pane: PaneId, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                woken.recv_timeout(left).is_ok(),
                "no update showed {text:?}"
            );
            let Some(update) = panes.take_update() else {
                continue;
            };
            for (id, screen) in update {
                let row: String = (0..screen.cols())
                    .filter_map(|col| screen.cell(0, col).map(|(text, _)| text.to_owned()))
                    .collect();
                if id == pane && row.starts_with(text) {
                    return;
                }
            }
        }
    }

    #[test]
    fn changes_made_while_an_update_waits_arrive_after_the_client_exits() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        let id = server
            .run(&["display-message", "-p", "-t", session, "#{pane_id}"])
            .unwrap();
        let pane = PaneId(id.trim().trim_start_matches('%').parse().unwrap());
        let layout = [PaneLayout {
            id: pane,
            left: 0,
            top: 0,
            cols: 200,
            rows: 50,
        }];
        let client = server.control(&["attach-session", "-t", session]).unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        // The first update waits, untaken, while more output arrives and the
        // client exits.
        panes.send_input(pane, b"first").unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        panes.send_input(pane, b" second").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        server.run(&["kill-server"]).unwrap();
        assert!(panes.take_update().is_some());
        wait_for_row(&panes, &woken, pane, "first second");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.is_finished() {
            assert!(Instant::now() < deadline, "the worker did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Returns the panes of the server's session.
    fn layout(server: &TmuxServer) -> Vec<PaneLayout> {
        let format = "#{pane_id} #{pane_left} #{pane_top} #{pane_width} #{pane_height}";
        let listed = server
            .run(&["list-panes", "-t", TmuxServer::SESSION, "-F", format])
            .unwrap();
        listed
            .lines()
            .map(|line| {
                let mut fields = line.split(' ');
                let id = fields.next().unwrap().trim_start_matches('%');
                let mut number = || fields.next().unwrap().parse().unwrap();
                PaneLayout {
                    id: PaneId(id.parse().unwrap()),
                    left: number(),
                    top: number(),
                    cols: number(),
                    rows: number(),
                }
            })
            .collect()
    }

    #[test]
    fn a_quiet_panes_change_joins_the_waiting_update() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        // Paced, so the echo does not queue behind a flood in a slow build.
        let stream = "while :; do echo streaming; sleep 0.02; done";
        server
            .run(&["split-window", "-v", "-t", session, "sh", "-c", stream])
            .unwrap();
        let layout = layout(&server);
        let quiet = layout[0].id;
        let client = server.control(&["attach-session", "-t", session]).unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        // An update holding the streaming pane waits, untaken, when the quiet
        // pane echoes a key.
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        panes.send_input(quiet, b"x").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let update = panes.take_update().unwrap();
        let echoed = update
            .iter()
            .any(|(id, screen)| *id == quiet && screen.cell(0, 0).is_some_and(|(t, _)| t == "x"));
        assert!(echoed, "the echo waited for the next update");
    }

    #[test]
    fn input_reaches_the_pane_and_its_echo_comes_back_as_an_update() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        let id = server
            .run(&["display-message", "-p", "-t", session, "#{pane_id}"])
            .unwrap();
        let pane = PaneId(id.trim().trim_start_matches('%').parse().unwrap());
        let layout = [PaneLayout {
            id: pane,
            left: 0,
            top: 0,
            cols: 200,
            rows: 50,
        }];
        let client = server.control(&["attach-session", "-t", session]).unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        panes.send_input(pane, b"hello").unwrap();
        wait_for_row(&panes, &woken, pane, "hello");
        assert!(!panes.is_finished());
        drop(panes);
        assert!(server.run(&["has-session", "-t", session]).is_ok());
    }
}
