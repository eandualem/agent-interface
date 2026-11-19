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

/// A sh function, `json`, that prints its argument as a JSON string. Fixture
/// values hold no line breaks or control characters.
const JSON_STRING: &str =
    r#"json() { printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')"; }"#;

/// `path` as one single-quoted sh word.
fn sh_quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', r"'\''"))
}

#[test]
fn ordinary_shell_creation_is_isolated_and_reuses_sessions() {
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
    std::fs::write(
        &shell,
        format!(
            r#"#!/bin/sh
{JSON_STRING}
args= separator=
for arg do args="$args$separator$(json "$arg")"; separator=,; done
environment= separator=
for name in TMUX TMUX_PANE TMUX_TMPDIR SSH_AUTH_SOCK XDG_CONFIG_HOME XDG_CONFIG_DIRS \
    XDG_DATA_HOME XDG_DATA_DIRS XDG_CACHE_HOME XDG_STATE_HOME XDG_RUNTIME_DIR FIXTURE_SECRET; do
    if eval "[ \"\${{$name+set}}\" ]"; then eval "value=\$$name"; value=$(json "$value"); else value=null; fi
    environment="$environment$separator\"$name\":$value"; separator=,
done
proof="${{0%/*}}/proof-$$.json"
printf '{{"cwd":%s,"args":[%s],"environment":{{%s}}}}' "$(json "$(pwd -P)")" "$args" "$environment" > "$proof.part"
mv "$proof.part" "$proof"
while :; do sleep 60; done
"#
        ),
    )
    .unwrap();
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
    let calls = root.join("failed-creation-calls.txt");
    let wrapper = tools.join("tmux");
    // Each call is recorded as one line: its exit status, then its arguments.
    let script = format!(
        r#"#!/bin/sh
count=$#
for arg do
    set -- "$@" "$arg"
    [ "$arg" != new-session ] || set -- "$@" -Z
done
shift "$count"
{tmux} "$@"
status=$?
printf '%s %s\n' "$status" "$*" >> {calls}
exit "$status"
"#,
        tmux = sh_quoted(&tmux),
        calls = sh_quoted(&calls),
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
    let calls = std::fs::read_to_string(&calls).unwrap();
    let calls: Vec<(&str, Vec<&str>)> = calls
        .lines()
        .map(|line| {
            let (status, args) = line.split_once(' ').unwrap();
            (status, args.split(' ').collect())
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].1.contains(&"list-sessions"));
    assert_eq!(calls[0].0, "0");
    assert!(calls[1].1.contains(&"new-session"));
    assert_ne!(calls[1].0, "0");
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

#[test]
fn creates_the_first_ordinary_session_on_a_fixture_owned_socket() {
    let server = TmuxServer::start_on_socket(|socket| {
        let parent = socket.parent().unwrap();
        let mut environment = Environment::capture()?;
        let shell = parent.join("namespace-shell");
        std::fs::write(
            &shell,
            format!(
                r#"#!/bin/sh
{JSON_STRING}
if [ "${{TMUX_TMPDIR+set}}" ]; then value=$(json "$TMUX_TMPDIR"); else value=null; fi
printf '{{"TMUX_TMPDIR":%s}}' "$value" > "$0.part"
mv "$0.part" "$0.json"
while :; do sleep 60; done
"#
            ),
        )?;
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
        // The command that starts the server is prepared by the caller.
        let environment = environment.prepared(|command| {
            command.env("TW_PREPARED", "1");
        });
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
    assert_eq!(
        server
            .run(&["show-environment", "-g", "TW_PREPARED"])
            .unwrap()
            .trim(),
        "TW_PREPARED=1"
    );
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

#[test]
fn a_second_creator_waits_for_the_first_creators_lock() {
    let server = TmuxServer::start().unwrap();
    let socket = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap();
    let socket = PathBuf::from(socket.trim());
    let root = server.address().0;
    let shell = root.join("idle-shell");
    std::fs::write(&shell, "#!/bin/sh\nwhile :; do sleep 60; done\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut environment = Environment::capture().unwrap();
    environment.shell = shell;
    environment.home = root.to_owned();
    environment.cwd = root.to_owned();
    for name in CONTEXT {
        environment.variables.remove(OsStr::new(name));
    }
    // flock conflicts between open file descriptions, so this one stands in
    // for another window that is creating shells.
    let held = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(format!("{}.viewer-lock", socket.display()))
        .unwrap();
    held.lock().unwrap();
    let creator = thread::spawn({
        let socket = socket.clone();
        let request = Request {
            id: "000000000001".into(),
            cwd: None,
        };
        move || ensure(&socket, &[request], &environment)
    });
    thread::sleep(Duration::from_millis(300));
    assert!(!creator.is_finished());
    assert!(
        server
            .run(&["has-session", "-t", "=terminal-000000000001"])
            .is_err()
    );
    held.unlock().unwrap();
    creator.join().unwrap().unwrap();
    let sessions = server
        .run(&["list-sessions", "-F", "#{session_name}"])
        .unwrap();
    assert_eq!(
        sessions
            .lines()
            .filter(|name| *name == "terminal-000000000001")
            .count(),
        1
    );
}

#[test]
fn a_shell_created_with_an_invalid_shell_variable_runs_bin_sh() {
    let server = TmuxServer::start().unwrap();
    let socket = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap();
    let socket = PathBuf::from(socket.trim());
    let root = server.address().0;
    // Keep the login shell away from the runner's profile and ENV files.
    server
        .run(&["set-environment", "-g", "HOME", root.to_str().unwrap()])
        .unwrap();
    for name in ["ENV", "BASH_ENV", "ZDOTDIR"] {
        server.run(&["set-environment", "-gu", name]).unwrap();
    }
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| name != "SHELL" && name != "HOME")
        .filter(|(name, _)| !CONTEXT.iter().any(|context| name == context))
        .collect();
    variables.push(("SHELL".into(), root.join("missing-shell").into()));
    variables.push(("HOME".into(), root.as_os_str().to_owned()));
    let environment = Environment::from_variables(variables, root.to_owned()).unwrap();
    ensure(
        &socket,
        &[Request {
            id: "000000000001".into(),
            cwd: None,
        }],
        &environment,
    )
    .unwrap();
    let start = server
        .run(&[
            "display-message",
            "-p",
            "-t",
            "=terminal-000000000001:",
            "#{pane_start_command}",
        ])
        .unwrap();
    // Each word of the start command is quoted for the shell.
    assert!(start.contains("'/bin/sh' '-l' '-i'"), "{start}");
    assert!(!start.contains("missing-shell"), "{start}");
}

#[test]
fn an_exited_ordinary_shell_is_kept_rather_than_restarted() {
    let server = TmuxServer::start().unwrap();
    let socket = server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap();
    let socket = PathBuf::from(socket.trim());
    let root = server.address().0;
    // It runs until the test creates its sentinel file, then exits.
    let shell = root.join("exiting-shell");
    let sentinel = root.join("exiting-shell.exit");
    std::fs::write(
        &shell,
        "#!/bin/sh\nwhile [ ! -e \"$0.exit\" ]; do sleep 0.05; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut environment = Environment::capture().unwrap();
    environment.shell = shell;
    environment.home = root.to_owned();
    environment.cwd = root.to_owned();
    for name in CONTEXT {
        environment.variables.remove(OsStr::new(name));
    }
    let request = || {
        [Request {
            id: "000000000001".into(),
            cwd: None,
        }]
    };
    ensure(&socket, &request(), &environment).unwrap();
    // ensure has set remain-on-exit, so the pane stays once the shell exits.
    std::fs::write(&sentinel, "").unwrap();
    let dead = || {
        server
            .run(&[
                "display-message",
                "-p",
                "-t",
                "=terminal-000000000001:",
                "#{pane_dead}",
            ])
            .unwrap()
            .trim()
            .to_owned()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while dead() != "1" {
        assert!(Instant::now() < deadline, "the shell did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    let exited = pid(&server, "000000000001");
    ensure(&socket, &request(), &environment).unwrap();
    assert_eq!(dead(), "1");
    assert_eq!(pid(&server, "000000000001"), exited);
    let sessions = server
        .run(&["list-sessions", "-F", "#{session_name}"])
        .unwrap();
    assert_eq!(
        sessions
            .lines()
            .filter(|name| *name == "terminal-000000000001")
            .count(),
        1
    );
}
