//! Gate 5's reference: an ordinary tmux client attached in a pseudo-terminal,
//! whose output the harness keeps (`docs/decisions/0021-pane-state-recovery.md`).

// Opening a pseudo-terminal and making it the client's controlling terminal
// need the C library.
#![allow(unsafe_code)]

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use tmux_workspaces_test_support::TmuxServer;

/// An ordinary tmux client in a pseudo-terminal, as a terminal would run it.
pub struct Reference {
    client: Child,
    master: File,
    bytes: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

impl Reference {
    /// Attaches an ordinary client to `session` on `server`, in a terminal of
    /// `cols` by `rows` cells that understands `xterm-256color`.
    pub fn attach(server: &TmuxServer, session: &str, cols: u16, rows: u16) -> io::Result<Self> {
        let (master, slave) = open(cols, rows)?;
        let line = server.shell_command(&["attach-session", "-E", "-t", session]);
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &format!("exec {line}")])
            .env("TERM", "xterm-256color")
            // tmux takes a client's terminal as UTF-8 when the first of
            // these that is set says so.
            .env_remove("LC_ALL")
            .env_remove("LC_CTYPE")
            .env("LANG", "en_US.UTF-8")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // SAFETY: the closure calls only async-signal-safe functions. The
        // pseudo-terminal becomes the client's controlling terminal, so
        // resizing it sends the client SIGWINCH, as a terminal's does.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let client = command.spawn()?;
        // Only the client may hold the terminal's other end, so that reading
        // ends when the client does.
        drop(command);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut reading = master.try_clone()?;
        let kept = Arc::clone(&bytes);
        let reader = std::thread::spawn(move || {
            let mut buffer = vec![0; 64 * 1024];
            loop {
                match reading.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => kept
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend_from_slice(&buffer[..read]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    // The client has exited.
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            client,
            master,
            bytes,
            reader: Some(reader),
        })
    }

    /// Write fixture input through the ordinary client's terminal, not send-keys.
    #[cfg(test)]
    pub fn send(&self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        (&self.master).write_all(bytes)
    }

    /// Returns how many bytes the client has written so far.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Returns the bytes the client has written from offset `from` on.
    pub fn bytes_from(&self, from: usize) -> Vec<u8> {
        self.lock().get(from..).unwrap_or_default().to_vec()
    }

    /// Resizes the terminal, as a person resizing its window does.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = winsize(cols, rows);
        // SAFETY: the descriptor is open and `size` outlives the call.
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.bytes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Reference {
    fn drop(&mut self) {
        // The client is the process the harness started: the shell replaced
        // itself with it.
        if let Ok(pid) = libc::pid_t::try_from(self.client.id()) {
            // SAFETY: signalling a child this process has not yet waited
            // for cannot reach another process.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        let _ = self.client.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Opens a pseudo-terminal of `cols` by `rows`, returning its controlling
/// and terminal ends. Neither is inherited by programs this process starts.
fn open(cols: u16, rows: u16) -> io::Result<(File, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let mut size = winsize(cols, rows);
    // SAFETY: the pointers are valid for the call; null name and terminal
    // settings are allowed.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two open descriptors that nothing else owns.
    let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        // SAFETY: the descriptor is open.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((master, slave))
}
