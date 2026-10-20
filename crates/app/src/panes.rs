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
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
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
    /// freshly attached with `-f no-output,ignore-size`. Its initial reply
    /// must not have been read. Captures existing contents before enabling
    /// live output; startup is bounded by a ten-second capture deadline.
    /// Each pane keeps about `scrollback` lines of history.
    ///
    /// `wake` is called on the worker thread whenever an update is ready, and
    /// once more when the worker has finished.
    pub fn start(
        mut client: Child,
        layout: &[PaneLayout],
        scrollback: usize,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let seed =
            match crate::seed::attach(&mut client, layout.iter().map(|pane| pane.id).collect()) {
                Ok(seed) => seed,
                Err(error) => {
                    let _ = client.kill();
                    let _ = client.wait();
                    return Err(error);
                }
            };
        let crate::seed::Seed {
            stdin,
            stdout,
            decoder,
            snapshots,
            output,
        } = seed;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queued: output.iter().map(|(_, bytes)| bytes.len()).sum(),
                queue: output.into(),
                ..State::default()
            }),
            changed: Condvar::new(),
        });
        let layout = layout.to_vec();
        // Should a thread fail to start, dropping `panes` stops what did.
        let mut panes = Self {
            shared,
            stdin: Mutex::new(Some(stdin)),
            client,
            threads: Vec::new(),
        };
        let reader = spawn("tmux reader", {
            let shared = Arc::clone(&panes.shared);
            move || read(&shared, stdout, decoder)
        });
        match reader {
            Ok(reader) => panes.threads.push(reader),
            Err(error) => {
                // Its output is closed unread, so the client might never exit.
                let _ = panes.client.kill();
                return Err(error);
            }
        }
        let (ready, initialized) = mpsc::channel();
        panes.threads.push(spawn("terminal worker", {
            let shared = Arc::clone(&panes.shared);
            move || work(&shared, &layout, snapshots, scrollback, &wake, ready)
        })?);
        initialized
            .recv_timeout(crate::seed::TIMEOUT)
            .map_err(|error| io::Error::other(format!("initializing terminal worker: {error}")))?
            .map_err(io::Error::other)?;
        Ok(panes)
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
fn read(shared: &Shared, mut stdout: ChildStdout, mut decoder: Decoder) {
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
    // A control client exits only once its output has been read: one whose
    // output is closed while it still has some to write stays attached and
    // keeps its server running (tmux 3.7c). So read to the end, discarding.
    while matches!(stdout.read(&mut buffer), Ok(read) if read > 0) {}
}

/// Applies queued output to each pane's terminal and publishes updates, until
/// the client has exited and the queue is empty.
fn work(
    shared: &Shared,
    layout: &[PaneLayout],
    snapshots: Vec<tmux_workspaces_tmux::recovery::Snapshot>,
    scrollback: usize,
    wake: &dyn Fn(),
    ready: mpsc::Sender<Result<(), String>>,
) {
    // Engine handles are not `Send`, so they are made on this thread.
    let terminals: Result<Vec<_>, _> = layout
        .iter()
        .map(|pane| {
            let snapshot = snapshots
                .iter()
                .find(|snapshot| snapshot.pane == pane.id)
                .ok_or_else(|| format!("no initial snapshot for pane %{}", pane.id.0))?;
            if (snapshot.state.width, snapshot.state.height) != (pane.cols, pane.rows) {
                return Err(format!(
                    "pane %{} changed size during attachment",
                    pane.id.0
                ));
            }
            if snapshot.state.cursor.0 >= snapshot.state.width {
                return Err(format!(
                    "pane %{} needs right-margin snapshot recovery before streaming",
                    pane.id.0
                ));
            }
            Terminal::restored(&crate::restore::restore(snapshot), scrollback)
                .map(|terminal| (pane.id, terminal))
                .map_err(|error| error.to_string())
        })
        .collect();
    let mut terminals = match terminals {
        Ok(terminals) => terminals,
        Err(error) => {
            shared.lock().finished = true;
            shared.changed.notify_all();
            let _ = ready.send(Err(error));
            wake();
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut changed: Vec<PaneId> = layout.iter().map(|pane| pane.id).collect();
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
        } else if !state.cancelled
            && let Some((pane, bytes)) = state.queue.pop_front()
        {
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

    #[test]
    fn attachment_publishes_existing_styles_before_applying_new_output() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        let signal = server.shell_command(&["wait-for", "-S", "seed-ready"]);
        let script =
            format!("printf '\\033[2J\\033[H\\033[45mSEEDED\\033[0m'; {signal}; exec sleep 60");
        server
            .run(&["respawn-pane", "-k", "-t", session, &script])
            .unwrap();
        server.run(&["wait-for", "seed-ready"]).unwrap();
        let layout = layout(&server);
        let pane = layout[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let update = panes.take_update().unwrap();
        let (_, screen) = update.iter().find(|(id, _)| *id == pane).unwrap();
        let text: String = (0..6).map(|col| screen.cell(0, col).unwrap().0).collect();
        assert_eq!(text, "SEEDED");
        assert_eq!(
            screen.cell(0, 0).unwrap().1.style.background,
            tmux_workspaces_term::Color::Palette(5)
        );
        panes.send_input(pane, b"more").unwrap();
        wait_for_row(&panes, &woken, pane, "SEEDEDmore");
    }

    #[test]
    fn an_unsafe_capture_hook_fails_attachment_without_stopping_the_server() {
        let server = TmuxServer::start().unwrap();
        server
            .run(&[
                "set-hook",
                "-g",
                "after-capture-pane",
                "display-message -p hooked",
            ])
            .unwrap();
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let result = Panes::start(client, &layout(&server), 100, || {});
        assert!(result.err().unwrap().to_string().contains("recovery hooks"));
        assert!(
            server
                .run(&["has-session", "-t", TmuxServer::SESSION])
                .is_ok()
        );
    }

    #[test]
    fn an_initial_margin_cursor_is_reported_instead_of_clamped() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        let signal = server.shell_command(&["wait-for", "-S", "seed-ready"]);
        let script = format!("printf '\\033[2J\\033[H%0200d' 0; {signal}; exec sleep 60");
        server
            .run(&["respawn-pane", "-k", "-t", session, &script])
            .unwrap();
        server.run(&["wait-for", "seed-ready"]).unwrap();
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size",
                "-t",
                session,
            ])
            .unwrap();
        let result = Panes::start(client, &layout(&server), 100, || {});
        assert!(result.err().unwrap().to_string().contains("right-margin"));
        assert!(server.run(&["has-session", "-t", session]).is_ok());
    }

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
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        wait_for_row(&panes, &woken, pane, "");
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
    fn a_cancelled_reader_reads_the_clients_output_to_its_end() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec yes streaming"])
            .unwrap();
        let mut client = server.control(&["attach-session", "-t", session]).unwrap();
        // The client's output fills its pipe while nothing reads it.
        std::thread::sleep(Duration::from_millis(500));
        let shared = Shared {
            state: Mutex::new(State {
                cancelled: true,
                ..State::default()
            }),
            changed: Condvar::new(),
        };
        let stdout = client.stdout.take().unwrap();
        drop(client.stdin.take());
        // Read on another thread, so that a client that never exits fails
        // the test instead of hanging it: killing the client ends the read.
        let reader = std::thread::spawn(move || read(&shared, stdout, Decoder::new()));

        let deadline = Instant::now() + Duration::from_secs(10);
        while client.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = client.kill();
                let _ = client.wait();
                panic!("the client did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        reader.join().unwrap();
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
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        wait_for_row(&panes, &woken, quiet, "");
        // An update holding the streaming pane waits, untaken, when the quiet
        // pane echoes a key. Only joining changes a waiting update, so without
        // it the echo never shows in this one.
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        panes.send_input(quiet, b"x").unwrap();
        let echoed = |update: &Update| {
            update.iter().any(|(id, screen)| {
                *id == quiet && screen.cell(0, 0).is_some_and(|(t, _)| t == "x")
            })
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.shared.lock().update.as_ref().is_some_and(echoed) {
            assert!(
                Instant::now() < deadline,
                "the echo waited for the next update"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
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
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size",
                "-t",
                session,
            ])
            .unwrap();
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
