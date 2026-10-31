//! A pane that starts an operating system command (OSC) and never ends it
//! must not make the terminal hold ever more memory.

use agent_interface_term::{Screen, Terminal};

/// Returns the most memory this process has held resident, in bytes.
fn peak_resident() -> u64 {
    // SAFETY: getrusage only writes the zeroed struct it is given.
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    let peak = u64::try_from(usage.ru_maxrss).unwrap();
    // macOS reports bytes; Linux reports kibibytes.
    if cfg!(target_os = "macos") {
        peak
    } else {
        peak * 1024
    }
}

#[test]
fn an_unterminated_clipboard_sequence_holds_bounded_memory() {
    let mut terminal = Terminal::new(10, 2, 0).unwrap();
    let before = peak_resident();
    terminal.write(b"\x1b]52;c;");
    let chunk = [b'A'; 64 * 1024];
    for _ in 0..1024 {
        terminal.write(&chunk);
    }
    let growth = peak_resident().saturating_sub(before);
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
