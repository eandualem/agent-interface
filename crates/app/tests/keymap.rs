//! Print-only keymap diagnostics use isolated files and no tmux process.

use std::{fs, process::Command};
use tmux_workspaces_core::keymap::Keymap;

#[test]
fn printing_uses_effective_keys_without_a_terminal_tmux_or_library() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let config = root.join("keys.toml");
    fs::write(&config, "prefix = 'C-b'\n[bindings]\nnew-tab = ['F2']").unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tmux-workspaces"));
        command
            .env_clear()
            .env("PATH", "")
            .env("HOME", root)
            .current_dir(root);
        command
    };
    let output = command()
        .args(["--keymap", "keys.toml", "--print-keymap", "toml"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let map = Keymap::parse(&output.stdout).unwrap();
    assert_eq!(map.prefix(), "C-b");
    assert_eq!(map.actions()[0].keys, ["F2"]);
    let output = command()
        .args(["--keymap=keys.toml", "--print-keymap=help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("F2                   New tab")
    );
    let output = command()
        .env("TMUX_WORKSPACES_KEYMAP", "missing")
        .args(["--no-keymap", "--print-keymap", "toml"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(Keymap::parse(&output.stdout).unwrap(), Keymap::default());
    for args in [
        vec!["--keymap", "missing", "--print-keymap", "toml"],
        vec![
            "--keymap",
            "keys.toml",
            "--no-keymap",
            "--print-keymap",
            "toml",
        ],
        vec!["--print-keymap", "ghostty"],
    ] {
        assert!(!command().args(args).output().unwrap().status.success());
    }
    assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "prefix = 'C-b'\n[bindings]\nnew-tab = ['F2']"
    );
}

#[test]
fn fifo_keymap_is_rejected_without_waiting_for_a_writer() {
    let fixture = tempfile::tempdir().unwrap();
    let fifo = fixture.path().join("keys.toml");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_tmux-workspaces"))
        .args(["--keymap"])
        .arg(&fifo)
        .args(["--print-keymap", "toml"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("keymap loading waited for a FIFO writer");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("not a regular file")
    );
}
