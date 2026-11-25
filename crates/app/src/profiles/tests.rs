use super::*;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};

/// `profile_id` as the Rust service lists it while serving it.
fn entry(profile_id: &str) -> Value {
    json!({
        "state": "serving", "protocol_version": 1, "profile_id": profile_id,
        "display_name": "Work", "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38"}, "socket": "/run/profile/api.sock",
        "tmux_socket": "/run/profile/tmux.sock", "credential_reference": {"type": "socket"}
    })
}

fn stub(script: &str) -> (tempfile::TempDir, Client) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("provider");
    // Executing a file that any process holds open for writing fails on Linux
    // with ETXTBSY, and a child another test thread forks while this one writes
    // inherits the descriptor. So `cp` writes the executed file, in its own
    // process, and this process never opens it for writing.
    let source = directory.path().join("provider.sh");
    fs::write(&source, format!("#!/bin/sh\n{script}\n")).unwrap();
    let copied = std::process::Command::new("/bin/cp")
        .arg(&source)
        .arg(&executable)
        .status()
        .unwrap();
    assert!(copied.success());
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    client.environment.push((
        "CAPTURE".into(),
        directory.path().join("calls").into_os_string(),
    ));
    client
        .environment
        .push(("HOME".into(), directory.path().as_os_str().to_owned()));
    (directory, client)
}

#[test]
fn discovery_only_lists_and_shows_records_without_connecting_or_mutating() {
    let list = json!({"profiles": [entry("profile-a")]});
    let ready = entry("profile-a");
    let failed = json!({"state": "failed", "profile_id": "-opaque;id", "display_name": "Work",
        "code": "start_failed", "reason": "The profile's folder cannot be read."});
    let script = format!(
        r#"
printf '%s\n' "$*" >> "$CAPTURE"
case "$*" in
  'profiles list --json') /bin/cat <<'FIXTURE'
{list}
FIXTURE
;;
  'profiles show --json -- profile-a') /bin/cat <<'FIXTURE'
{ready}
FIXTURE
;;
  'profiles show --json -- -opaque;id') /bin/cat <<'FIXTURE'
{failed}
FIXTURE
;;
  *) exit 9;;
esac
"#
    );
    let (directory, client) = stub(&script);
    let items = client.list(&|| true).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].profile_id, "profile-a");
    let profile = client.show("profile-a", &|| true).unwrap();
    assert_eq!(profile.item().state, profiles::State::Ready);
    let failed = client.show("-opaque;id", &|| true).unwrap();
    assert_eq!(failed.item().state, profiles::State::Failed);
    assert_eq!(
        failed.item().reason.as_deref(),
        Some("The profile's folder cannot be read.")
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "profiles list --json\nprofiles show --json -- profile-a\nprofiles show --json -- -opaque;id\n"
    );
    // The entry points at nonexistent example paths. Showing it succeeds
    // because discovery never connects.
}

#[test]
fn discovery_preserves_operator_refusals_and_never_returns_failed_output() {
    let (directory, mut client) = stub(
        r#"
if [ "$BACKBONE_AGENT" = fixture-agent ]; then
  printf refused > "$CAPTURE"
  exit 7
fi
printf '{"profiles":[]}'
"#,
    );
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "fixture-agent".into()));
    assert!(matches!(client.list(&|| true), Err(Error::CommandFailed)));
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "refused"
    );
    let (_directory, client) = stub(r#"printf '{"profiles":[]}'; exit 2"#);
    assert!(matches!(client.list(&|| true), Err(Error::CommandFailed)));
}

#[test]
fn discovery_rejects_wrong_profiles_and_invalid_or_oversized_output() {
    let ready = entry("profile-a");
    let (_directory, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{ready}\nFIXTURE"));
    let shown = client.show("another-profile", &|| true);
    assert!(
        matches!(shown, Err(Error::Profile(profiles::Error::BindingMismatch))),
        "{shown:?}"
    );
    assert!(matches!(client.show("", &|| true), Err(Error::Invalid)));
    assert!(matches!(
        client.show("two words", &|| true),
        Err(Error::Invalid)
    ));
    let mut later = entry("profile-a");
    later["protocol_version"] = json!(2);
    let later = json!({"profiles": [later]});
    let (_directory, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{later}\nFIXTURE"));
    assert!(matches!(
        client.list(&|| true),
        Err(Error::Profile(profiles::Error::Unsupported))
    ));
    let (_directory, client) = stub("printf '{broken'");
    assert!(matches!(
        client.list(&|| true),
        Err(Error::Profile(profiles::Error::Invalid))
    ));
    let (_directory, client) = stub(&format!("/usr/bin/head -c {} /dev/zero", OUTPUT_LIMIT + 1));
    assert!(matches!(client.list(&|| true), Err(Error::TooLarge)));
    assert!(matches!(
        Client::new("relative-provider".into(), "/bin".into()),
        Err(Error::Invalid)
    ));
}

#[test]
fn discovery_deadlines_and_cancellation_stop_the_original_child() {
    let (directory, client) = stub(r#"printf started > "$CAPTURE"; exec /bin/sleep 5"#);
    assert!(matches!(client.list(&|| false), Err(Error::Cancelled)));
    assert!(!directory.path().join("calls").exists());
    let start = Instant::now();
    assert_eq!(
        client.read(
            &["profiles", "list", "--json"],
            &|| true,
            Duration::from_millis(40)
        ),
        Err(Error::TimedOut)
    );
    assert!(start.elapsed() < Duration::from_secs(4));
    let start = Instant::now();
    assert_eq!(
        client.read(
            &["profiles", "list", "--json"],
            &|| start.elapsed() < Duration::from_millis(40),
            Duration::from_secs(4)
        ),
        Err(Error::Cancelled)
    );
    assert!(start.elapsed() < Duration::from_secs(4));
}

#[test]
fn discovery_cleans_wrapper_children_on_cancellation_timeout_and_success() {
    for mode in ["cancel", "timeout", "success"] {
        let script = r#"
/bin/sh -c 'printf "%s" "$$" > "$CAPTURE.pid"; while :; do printf x >> "$CAPTURE.beats"; /bin/sleep 0.01; done' &
"#.to_owned() + if mode == "success" {
            r#"while [ ! -s "$CAPTURE.beats" ]; do /bin/sleep 0.01; done
printf '{"profiles":[]}'
"#
        } else { "wait\n" };
        let (directory, client) = stub(&script);
        let beats = directory.path().join("calls.beats");
        let result = client.read(
            &["profiles", "list", "--json"],
            &|| mode != "cancel" || !beats.exists(),
            // The others end at the first beat, which can take seconds under load.
            Duration::from_secs(if mode == "timeout" { 2 } else { 30 }),
        );
        // Under load the timeout can stop the child before its first beat.
        let length = || match fs::metadata(&beats) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            other => other.unwrap().len(),
        };
        let before = length();
        std::thread::sleep(Duration::from_millis(120));
        let after = length();
        if after != before {
            // Clean up the fixture even when testing a regressed implementation.
            // Growth proves this never-ending child still owns the recorded PID.
            let pid = fs::read_to_string(directory.path().join("calls.pid"))
                .unwrap()
                .parse()
                .unwrap();
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_raw(pid).unwrap(),
                rustix::process::Signal::KILL,
            );
        }
        assert_eq!(after, before, "wrapper child survived {mode}");
        match mode {
            "cancel" => assert_eq!(result, Err(Error::Cancelled)),
            "timeout" => assert_eq!(result, Err(Error::TimedOut)),
            "success" => assert!(result.is_ok()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn an_unbounded_writer_hits_the_pipe_collection_limit() {
    let (_directory, client) = stub("exec /usr/bin/yes fixture");
    assert_eq!(
        client.read(
            &["profiles", "list", "--json"],
            &|| true,
            Duration::from_secs(5)
        ),
        Err(Error::TooLarge)
    );
}

/// A first agent `name` of the profile `profile_id`, which the Rust service
/// serves, under `request_id`, with the choices of the service's
/// first-agent fixture.
pub(super) fn first_agent(
    request_id: &str,
    profile_id: &str,
    name: &str,
) -> profiles::setup::Prepared {
    use profiles::setup::{Agent, Brief, Prepared};
    let entry = json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": profile_id, "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": "/run/profile/api.sock", "tmux_socket": "/run/profile/tmux.sock",
        "credential_reference": {"type": "socket"}
    });
    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    let agent = Agent {
        name: name.into(),
        directory: "/home/user/projects/site".into(),
        create_directory: true,
        runtime: "codex".into(),
        model: None,
        brief: Brief::Template {
            template: "orchestrator-v1".into(),
        },
        orchestrator: false,
    };
    Prepared::first_agent(request_id, &profile, agent, true).unwrap()
}
/// The `n`th request ID, of the form the service takes.
pub(super) fn request(n: u8) -> String {
    format!("019a8f3f1a2b7c3d8e4f5a6b7c8d9e{n:02x}")
}
/// The ready result of the agent `guide`, built in the roster crate from the
/// setup guide, for `request_id`.
pub(super) fn ready(request_id: &str) -> Value {
    let mut value: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/src/profiles/setup/fixtures/first-agent-result.json"
    )))
    .unwrap();
    value["request_id"] = request_id.into();
    value
}
/// A result of `action` for `request_id`, with no data.
pub(super) fn answer(
    request_id: &str,
    action: &str,
    status: &str,
    code: &str,
    retry: &str,
) -> Value {
    json!({"schema_version": 3, "request_id": request_id, "seq": 1, "type": "result",
        "action": action, "status": status, "code": code, "message": "", "data": {},
        "completed_steps": [], "retry": retry})
}
/// The setup guide's one answer that confirms a cancellation.
pub(super) fn cancelled(request_id: &str) -> Value {
    let mut value = answer(
        request_id,
        "cancel_first_agent",
        "completed",
        "request_cancelled",
        "new_request",
    );
    value["data"] = json!({"cancelled_request_id": request_id, "can_start_new_request": true});
    value
}
/// A completed step, as the command prints it once the profile answered.
pub(super) fn step(request_id: &str, seq: u64, step: &str) -> Value {
    json!({"schema_version": 3, "request_id": request_id, "seq": seq, "type": "step",
        "step": step, "status": "completed"})
}
