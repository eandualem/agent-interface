//! A pane that starts an operating system command (OSC) and never ends it
//! must not make the terminal hold ever more memory.

use agent_interface_term::{Screen, Terminal};

/// Returns the memory this process holds resident now, in bytes. A peak
/// reading would not do: on Linux it carries over from the process that
/// launched the test.
fn resident() -> u64 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_pidinfo only writes the zeroed struct it is given.
        let mut info = unsafe { std::mem::zeroed::<libc::proc_taskinfo>() };
        let size = libc::c_int::try_from(std::mem::size_of_val(&info)).unwrap();
        let written = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                std::ptr::from_mut(&mut info).cast(),
                size,
            )
        };
        assert_eq!(written, size);
        info.pti_resident_size
    }
    #[cfg(not(target_os = "macos"))]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
        let pages: u64 = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
        // SAFETY: sysconf has no preconditions.
        let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap();
        pages * page
    }
}

#[test]
fn an_unterminated_clipboard_sequence_holds_bounded_memory() {
    let mut terminal = Terminal::new(10, 2, 0).unwrap();
    let before = resident();
    terminal.write(b"\x1b]52;c;");
    let chunk = [b'A'; 64 * 1024];
    for _ in 0..1024 {
        terminal.write(&chunk);
    }
    // The sequence is still open, so anything it captured is still held.
    let growth = resident().saturating_sub(before);
    assert!(
        growth < 16 << 20,
        "64 MiB of one unterminated sequence grew memory by {growth} bytes"
    );

    // The discarded sequence still ends, and the pane draws what follows.
    terminal.write(b"\x07ok");
    let mut screen = Screen::default();
    terminal.read_screen(&mut screen).unwrap();
    let text: String = (0..2).map(|col| screen.cell(0, col).unwrap().0).collect();
    assert_eq!(text, "ok");
}
