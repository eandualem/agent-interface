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
