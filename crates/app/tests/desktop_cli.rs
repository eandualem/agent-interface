//! Diagnostic CLI paths must not start the native window or create a library.

#[test]
fn diagnostics_run_without_terminal_or_library() {
    let directory = tempfile::tempdir().unwrap();
    for flag in ["--help", "--version", "--unknown"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_tmux-workspaces"))
            .arg(flag)
            .env_clear()
            .env("HOME", directory.path())
            .env("PATH", "")
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert_eq!(output.status.success(), flag != "--unknown");
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn browser_without_a_display_exits_instead_of_polling_an_invisible_window() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tmux-workspaces"))
        .env_clear()
        .env("HOME", directory.path())
        .env("PATH", "")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("headless browser did not exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires a graphical desktop"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
