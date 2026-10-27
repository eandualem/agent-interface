use super::*;
use agent_interface_test_support::TmuxServer;
use std::os::unix::fs::PermissionsExt;

fn wait_file(path: &Path) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return value;
        }
        assert!(Instant::now() < deadline, "fixture receipt did not arrive");
        thread::sleep(Duration::from_millis(10));
    }
}

fn pid(server: &TmuxServer, id: &str) -> String {
    server
        .run(&[
            "display-message",
            "-p",
            "-t",
            &format!("=terminal-{id}:"),
            "#{pane_pid}",
        ])
        .unwrap()
        .trim()
        .to_owned()
}

// This same test entry point is a subprocess fixture for the separately recorded
// Python/Rust interoperability checks. Ordinary test runs take the parent branch.
#[test]
fn ordinary_shell_creation_is_isolated_and_reuses_sessions() {
    if let Some(input) = std::env::var_os("TW_SHELL_FIXTURE_INPUT") {
        let input: serde_json::Value =
            serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let socket = Path::new(input["socket"].as_str().unwrap());
        assert!(
            socket.ancestors().any(|part| part
                .file_name()
                .is_some_and(|name| name.as_bytes().starts_with(b"tw-tmux-"))),
            "subprocess fixtures require a TmuxServer-owned private namespace"
        );
        if let Some(ready) = input["hold_ready"].as_str() {
            let lock = OpenOptions::new()
                .append(true)
                .create(true)
                .open(format!("{}.viewer-lock", socket.display()))
                .unwrap();
            lock.lock().unwrap();
            std::fs::write(ready, b"true").unwrap();
            let release = Path::new(input["hold_release"].as_str().unwrap());
            let deadline = Instant::now() + Duration::from_secs(10);
            while !release.exists() {
                assert!(
                    Instant::now() < deadline,
                    "fixture lock release did not arrive"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
        let requests: Vec<_> = input["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|request| Request {
                id: request["id"].as_str().unwrap().into(),
                cwd: request["cwd"].as_str().map(PathBuf::from),
            })
            .collect();
        let environment = Environment::capture().unwrap();
        if let Some(started) = input["started"].as_str() {
            std::fs::write(started, b"true").unwrap();
        }
        let result = ensure(socket, &requests, &environment);
        let mut pids = BTreeMap::new();
        if result.is_ok() {
            for request in &requests {
                let output = environment
                    .run(
                        socket,
                        &[
                            "display-message".into(),
                            "-p".into(),
                            "-t".into(),
                            format!("=terminal-{}:", request.id).into(),
                            "#{pane_pid}".into(),
                        ],
                    )
                    .unwrap();
                require_success(output.clone()).unwrap();
                pids.insert(
                    request.id.clone(),
                    String::from_utf8(output.stdout).unwrap().trim().to_owned(),
                );
            }
        }
        let receipt = serde_json::json!({"ok":result.is_ok(),"pids":pids,"error":result.as_ref().err().map(ToString::to_string)});
        std::fs::write(input["receipt"].as_str().unwrap(), receipt.to_string()).unwrap();
        result.unwrap();
        return;
    }

    let server = TmuxServer::start().unwrap();
    // Only this disposable server is changed. Use a non-login POSIX command
    // interpreter so the synthetic shell never involves user shell startup files.
    server
        .run(&["set-option", "-g", "default-shell", "/bin/sh"])
        .unwrap();
    let socket = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap();
    let socket = Path::new(socket.trim_end());
    let root = server.address().0;
    let cwd = root.join("ordinary #{session_name} ## path;");
    let home = root.join("home");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir(&home).unwrap();
    // A tmux client HOME does not replace an existing server HOME. Keep real
    // fallback shells away from the runner's profile and ENV startup files.
    server
        .run(&["set-environment", "-g", "HOME", home.to_str().unwrap()])
        .unwrap();
    for name in ["ENV", "BASH_ENV", "ZDOTDIR"] {
        server.run(&["set-environment", "-gu", name]).unwrap();
    }
    let shell = root.join("fixture ' shell;");
    std::fs::write(&shell, r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
names = ['TMUX','TMUX_PANE','TMUX_TMPDIR','SSH_AUTH_SOCK','XDG_CONFIG_HOME','XDG_CONFIG_DIRS','XDG_DATA_HOME','XDG_DATA_DIRS','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','FIXTURE_SECRET']
path = pathlib.Path(__file__).parent / ('proof-' + str(os.getpid()) + '.json')
path.write_text(json.dumps({'cwd':os.getcwd(),'args':sys.argv[1:],'environment':{name:os.environ.get(name) for name in names}}))
while True: time.sleep(60)
"#).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut environment = Environment::capture().unwrap();
    environment.shell = shell.clone();
    environment.home = home.canonicalize().unwrap();
    environment.cwd = cwd.canonicalize().unwrap();
    for name in CONTEXT {
        environment.variables.remove(OsStr::new(name));
    }
    environment
        .variables
        .insert("SSH_AUTH_SOCK".into(), "fixture socket ' ; value".into());
    environment
        .variables
        .insert("XDG_CONFIG_HOME".into(), "".into());
    environment
        .variables
        .insert("FIXTURE_SECRET".into(), "must-not-pass".into());
    server
        .run(&[
            "set-environment",
            "-g",
            "SSH_AUTH_SOCK",
            "stale-server-socket",
        ])
        .unwrap();
    server
        .run(&[
            "set-environment",
            "-g",
            "XDG_DATA_HOME",
            "stale-server-data",
        ])
        .unwrap();
    server
        .run(&["set-environment", "-gu", "FIXTURE_SECRET"])
        .unwrap();
    let request = Request {
        id: "000000000001".into(),
        cwd: Some(cwd.clone()),
    };
    ensure(socket, &[request], &environment).unwrap();
    let first_pid = pid(&server, "000000000001");
    let proof = wait_file(&root.join(format!("proof-{first_pid}.json")));
    assert_eq!(proof["cwd"], cwd.canonicalize().unwrap().to_str().unwrap());
    assert_eq!(proof["args"], serde_json::json!(["-l", "-i"]));
    assert!(proof["environment"]["TMUX"].is_null());
    assert!(proof["environment"]["TMUX_PANE"].is_null());
    assert!(proof["environment"]["TMUX_TMPDIR"].is_null());
    assert_eq!(
        proof["environment"]["SSH_AUTH_SOCK"],
        "fixture socket ' ; value"
    );
    assert_eq!(proof["environment"]["XDG_CONFIG_HOME"], "");
    assert!(proof["environment"]["XDG_DATA_HOME"].is_null());
    assert!(proof["environment"]["FIXTURE_SECRET"].is_null());
    for (command, option, expected) in [
        ("show-options", "status", "off"),
        ("show-options", "mouse", "on"),
        ("show-window-options", "remain-on-exit", "on"),
    ] {
        assert_eq!(
            server
                .run(&[command, "-v", "-t", "=terminal-000000000001:", option])
                .unwrap()
                .trim(),
            expected
        );
    }
    environment.variables.remove(OsStr::new("SSH_AUTH_SOCK"));
    environment
        .variables
        .insert("XDG_CONFIG_HOME".into(), "new-context".into());
    ensure(
        socket,
        &[
            Request {
                id: "000000000001".into(),
                cwd: Some(home.clone()),
            },
            Request {
                id: "000000000002".into(),
                cwd: Some(root.join("missing")),
            },
        ],
        &environment,
    )
    .unwrap();
    assert_eq!(pid(&server, "000000000001"), first_pid);
    let second_pid = pid(&server, "000000000002");
    assert_ne!(second_pid, first_pid);
    let second = wait_file(&root.join(format!("proof-{second_pid}.json")));
    assert_eq!(
        second["cwd"],
        home.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(second["environment"]["SSH_AUTH_SOCK"].is_null());
    assert_eq!(second["environment"]["XDG_CONFIG_HOME"], "new-context");
    assert_eq!(
        wait_file(&root.join(format!("proof-{first_pid}.json"))),
        proof
    );
    if let Some(worker) = std::env::var_os("TW_PYTHON_SHELL_WORKER") {
        interoperability(&server, socket, &environment, Path::new(&worker));
    }
    // Force a real new-session command error after a successful list-sessions,
    // then retry through the unchanged shared lock file.
    let lock_path = format!("{}.viewer-lock", socket.display());
    let before = std::fs::metadata(&lock_path).unwrap();
    let original_path = environment.variables[OsStr::new("PATH")].clone();
    let tmux = std::env::split_paths(&original_path)
        .map(|path| path.join("tmux"))
        .find(|path| path.is_file())
        .unwrap();
    let tools = root.join("failure-tools");
    std::fs::create_dir(&tools).unwrap();
    let calls = root.join("failed-creation-calls.jsonl");
    let wrapper = tools.join("tmux");
    let script = format!(
        r#"#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
if 'new-session' in args:
    args.insert(args.index('new-session') + 1, '-Z')
result = subprocess.run([{tmux}] + args)
with open({calls}, 'a') as stream:
    stream.write(json.dumps({{'args': args, 'returncode': result.returncode}}) + '\n')
sys.exit(result.returncode)
"#,
        tmux = serde_json::to_string(tmux.to_str().unwrap()).unwrap(),
        calls = serde_json::to_string(calls.to_str().unwrap()).unwrap()
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut path = tools.as_os_str().to_owned();
    path.push(":");
    path.push(&original_path);
    environment.variables.insert("PATH".into(), path);
    let retry = [Request {
        id: "000000000003".into(),
        cwd: None,
    }];
    let error = ensure(socket, &retry, &environment).unwrap_err();
    assert!(error.to_string().contains("Ordinary-shell command failed"));
    let calls: Vec<serde_json::Value> = std::fs::read_to_string(&calls)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[0]["args"]
            .as_array()
            .unwrap()
            .contains(&"list-sessions".into())
    );
    assert_eq!(calls[0]["returncode"], 0);
    assert!(
        calls[1]["args"]
            .as_array()
            .unwrap()
            .contains(&"new-session".into())
    );
    assert_ne!(calls[1]["returncode"], 0);
    let lock = OpenOptions::new().append(true).open(&lock_path).unwrap();
    lock.try_lock().unwrap();
    assert_eq!(lock.metadata().unwrap().ino(), before.ino());
    drop(lock);
    environment.variables.insert("PATH".into(), original_path);
    ensure(socket, &retry, &environment).unwrap();
    assert!(!pid(&server, "000000000003").is_empty());
    let after = std::fs::metadata(&lock_path).unwrap();
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert!(server.run(&["list-clients"]).unwrap().is_empty());
}

fn interoperability(
    server: &TmuxServer,
    socket: &Path,
    environment: &Environment,
    python_worker: &Path,
) {
    use std::process::{Child, Stdio};
    let root = server.address().0;
    fn finish(mut child: Child) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("fixture worker timed out");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    let spawn = |rust: bool, input: &Path, environment: &Environment| {
        let mut command = if rust {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "shells::tests::ordinary_shell_creation_is_isolated_and_reuses_sessions",
                "--nocapture",
            ]);
            command.env("TW_SHELL_FIXTURE_INPUT", input);
            command
        } else {
            let mut command = Command::new("python3");
            command.arg(python_worker).arg(input);
            command
        };
        // Preserve only the explicit synthetic launcher context. The Python worker
        // adds TMUX_TMPDIR to the reference allowlist solely for fixture isolation.
        let fixture_input = rust.then(|| input.as_os_str().to_owned());
        command
            .env_clear()
            .envs(&environment.variables)
            .env("SHELL", &environment.shell)
            .env("HOME", &environment.home)
            .env("TMUX_TMPDIR", root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(input) = fixture_input {
            command.env("TW_SHELL_FIXTURE_INPUT", input);
        }
        command.spawn().unwrap()
    };
    let mut receipts = Vec::new();
    for (round, rust_holds) in [false, true].into_iter().enumerate() {
        let mut context = environment.clone();
        for name in CONTEXT {
            context.variables.remove(OsStr::new(name));
        }
        if round == 0 {
            context
                .variables
                .insert("SSH_AUTH_SOCK".into(), "shared socket ; ' value".into());
            context
                .variables
                .insert("XDG_CONFIG_HOME".into(), "".into());
        } else {
            context.variables.insert("XDG_DATA_HOME".into(), "".into());
        }
        let id = format!("00000000001{round}");
        let ready = root.join(format!("ready-{round}"));
        let release = root.join(format!("release-{round}"));
        let holder_receipt = root.join(format!("holder-{round}.json"));
        let waiter_receipt = root.join(format!("waiter-{round}.json"));
        let mut requests = vec![serde_json::json!({"id":id,"cwd":environment.cwd})];
        if round == 1 {
            requests.push(serde_json::json!({"id":"000000000010","cwd":environment.home}));
        }
        let holder = serde_json::json!({"socket":socket,"requests":requests,"receipt":holder_receipt,"hold_ready":ready,"hold_release":release});
        let waiter_started = root.join(format!("waiter-started-{round}.json"));
        let waiter = serde_json::json!({"socket":socket,"requests":requests,"receipt":waiter_receipt,"started":waiter_started});
        let holder_input = root.join(format!("holder-input-{round}.json"));
        let waiter_input = root.join(format!("waiter-input-{round}.json"));
        std::fs::write(&holder_input, holder.to_string()).unwrap();
        std::fs::write(&waiter_input, waiter.to_string()).unwrap();
        let holder = spawn(rust_holds, &holder_input, &context);
        wait_file(&ready);
        let waiter = spawn(!rust_holds, &waiter_input, &context);
        wait_file(&waiter_started);
        thread::sleep(Duration::from_millis(150));
        assert!(
            !waiter_receipt.exists(),
            "the other implementation must wait for flock"
        );
        assert!(
            server
                .run(&["has-session", "-t", &format!("=terminal-{id}:")])
                .is_err()
        );
        std::fs::write(&release, b"release").unwrap();
        finish(holder);
        finish(waiter);
        let a = wait_file(&holder_receipt);
        let b = wait_file(&waiter_receipt);
        assert_eq!(a["ok"], true);
        assert_eq!(b["ok"], true);
        assert_eq!(a["pids"], b["pids"]);
        let created_pid = pid(server, &id);
        assert_eq!(a["pids"][&id], created_pid);
        let proof = wait_file(&root.join(format!("proof-{created_pid}.json")));
        assert_eq!(proof["cwd"], environment.cwd.to_str().unwrap());
        if round == 0 {
            assert_eq!(
                proof["environment"]["SSH_AUTH_SOCK"],
                "shared socket ; ' value"
            );
            assert_eq!(proof["environment"]["XDG_CONFIG_HOME"], "");
        } else {
            assert!(proof["environment"]["SSH_AUTH_SOCK"].is_null());
            assert!(proof["environment"]["XDG_CONFIG_HOME"].is_null());
            assert_eq!(proof["environment"]["XDG_DATA_HOME"], "");
            let original_pid = pid(server, "000000000010");
            let original = wait_file(&root.join(format!("proof-{original_pid}.json")));
            assert_eq!(
                original["environment"]["SSH_AUTH_SOCK"],
                "shared socket ; ' value"
            );
            assert_ne!(original_pid, created_pid);
        }
        receipts.push(serde_json::json!({"rust_holds":rust_holds,"holder":a,"waiter":b,"proof":proof,"blocked_until_release":true}));
    }
    // Both implementations resolve an invalid SHELL and cwd using the same fallbacks.
    let mut fallback = environment.clone();
    fallback.shell = root.join("missing-shell");
    let request = root.join("fallback-input.json");
    let receipt = root.join("fallback-receipt.json");
    let value = serde_json::json!({"socket":socket,"requests":[{"id":"000000000012","cwd":root.join("missing-cwd")}],"receipt":receipt});
    std::fs::write(&request, value.to_string()).unwrap();
    finish(spawn(true, &request, &fallback));
    let before = wait_file(&receipt);
    finish(spawn(false, &request, &fallback));
    assert_eq!(wait_file(&receipt)["pids"], before["pids"]);
    assert_eq!(
        server
            .run(&[
                "display-message",
                "-p",
                "-t",
                "=terminal-000000000012:",
                "#{pane_current_path}"
            ])
            .unwrap()
            .trim(),
        environment.home.to_str().unwrap()
    );
    assert!(
        server
            .run(&[
                "display-message",
                "-p",
                "-t",
                "=terminal-000000000012:",
                "#{pane_start_command}"
            ])
            .unwrap()
            .contains("/bin/sh")
    );
    server
        .run(&[
            "send-keys",
            "-t",
            "=terminal-000000000012:",
            "exit",
            "Enter",
        ])
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while server
        .run(&[
            "display-message",
            "-p",
            "-t",
            "=terminal-000000000012:",
            "#{pane_dead}",
        ])
        .unwrap()
        .trim()
        != "1"
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    finish(spawn(false, &request, environment));
    finish(spawn(true, &request, environment));
    assert_eq!(wait_file(&receipt)["pids"], before["pids"]);
    assert_eq!(
        server
            .run(&[
                "display-message",
                "-p",
                "-t",
                "=terminal-000000000012:",
                "#{pane_dead}"
            ])
            .unwrap()
            .trim(),
        "1"
    );
    if let Some(output) = std::env::var_os("TW_SHELL_INTEROP_RECEIPT") {
        std::fs::write(output, serde_json::json!({"lock_rounds":receipts,"fallback_and_dead_reuse":true,"fallback_pid":before["pids"]}).to_string()).unwrap();
    }
}

#[test]
fn creates_the_first_ordinary_session_on_a_fixture_owned_socket() {
    let server = TmuxServer::start_on_socket(|socket| {
        let parent = socket.parent().unwrap();
        let mut environment = Environment::capture()?;
        let shell = parent.join("namespace-shell");
        std::fs::write(&shell, r#"#!/usr/bin/env python3
import json, os, pathlib, time
pathlib.Path(__file__).with_suffix('.json').write_text(json.dumps({'TMUX_TMPDIR': os.environ.get('TMUX_TMPDIR')}))
while True: time.sleep(60)
"#)?;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700))?;
        environment.shell = shell;
        environment.home = parent.to_owned();
        environment.cwd = parent.to_owned();
        environment
            .variables
            .insert("SHELL".into(), "/bin/sh".into());
        environment
            .variables
            .insert("HOME".into(), parent.as_os_str().to_owned());
        for name in CONTEXT {
            environment.variables.remove(OsStr::new(name));
        }
        ensure(
            socket,
            &[Request {
                id: "000000000001".into(),
                cwd: None,
            }],
            &environment,
        )
    })
    .unwrap();
    let socket = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap();
    let namespace = Path::new(socket.trim())
        .parent()
        .unwrap()
        .join("namespace-shell.json");
    assert!(wait_file(&namespace)["TMUX_TMPDIR"].is_null());
    assert_eq!(
        server
            .run(&["list-sessions", "-F", "#{session_name}"])
            .unwrap()
            .trim(),
        "terminal-000000000001"
    );
    assert_eq!(
        server
            .run(&[
                "show-window-options",
                "-v",
                "-t",
                "=terminal-000000000001:",
                "remain-on-exit"
            ])
            .unwrap()
            .trim(),
        "on"
    );
    assert!(server.run(&["list-clients"]).unwrap().is_empty());
}

#[test]
fn fifo_lock_is_rejected_without_waiting_for_a_reader() {
    let server = TmuxServer::start().unwrap();
    let socket = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap();
    let socket = Path::new(socket.trim());
    let lock = format!("{}.viewer-lock", socket.display());
    assert!(
        Command::new("mkfifo")
            .arg(&lock)
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    assert!(
        ensure(
            socket,
            &[Request {
                id: "000000000001".into(),
                cwd: None
            }],
            &Environment::capture().unwrap()
        )
        .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}
