use super::*;
use crate::roster::content::tests::{Reply, provider};
use serde_json::{Value, json};

const REG: &str = "1234567890abcdef1234567890abcdef";
fn inspect(label: &str) -> Value {
    json!({"name":"worker","registration_id":REG,"runtime":"codex","known":true,"repo":format!("sentinel-{label}")})
}
fn permissions(label: &str) -> Value {
    json!({"schema_version":1,"profile_id":format!("profile-{label}"),"installation_id":format!("installation-{label}"),"lifecycle_id":format!("lifecycle-{label}"),"agent_name":"worker","registration_id":REG,"runtime":"codex","permissions":{"status":"available","files":[],"allow":[format!("sentinel-{label}")],"deny":[],"ask":[],"modes":[],"error":null}})
}
fn reader(connection: Connection) -> Reader {
    Reader {
        connection,
        name: "worker".into(),
        registration: REG.into(),
        target: None,
    }
}

#[test]
fn native_cards_retain_registration_credentials_and_delayed_profile_origin() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let context_path = format!("/api/agents/worker/runtime-context?expected_registration_id={REG}");
    let (arrived, waiting) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let (_a, a, server_a) = provider(
        "a",
        vec![
            Reply {
                delay: Some((arrived, held)),
                ..Reply::ok(&inspect_path, inspect("a"))
            },
            Reply::ok(&context_path, permissions("a")),
            Reply::ok(&inspect_path, inspect("a")),
            Reply::ok(&context_path, permissions("a")),
            Reply {
                status: 409,
                ..Reply::ok(
                    &inspect_path,
                    json!({"detail":{"code":"registration_changed"}}),
                )
            },
        ],
    );
    let (_b, b, server_b) = provider(
        "b",
        vec![
            Reply::ok(&inspect_path, inspect("b")),
            Reply::ok(&context_path, permissions("b")),
        ],
    );
    let credential = a.credential_reference().to_owned();
    let worker = std::thread::spawn(move || {
        let mut reader = reader(a);
        let snapshot = reader.read().unwrap();
        (reader, snapshot)
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    std::fs::write(credential, "BACKBONE_API_KEY=rotated\n").unwrap();
    let snapshot_b = reader(b).read().unwrap();
    release.send(()).unwrap();
    let (mut reader_a, snapshot_a) = worker.join().unwrap();
    assert_eq!(snapshot_a.agent.repo, "sentinel-a");
    assert_eq!(snapshot_a.permissions.allow, ["sentinel-a"]);
    assert_eq!(snapshot_b.agent.repo, "sentinel-b");
    assert_eq!(reader_a.read().unwrap().agent.repo, "sentinel-a");
    assert!(matches!(
        reader_a.read(),
        Err(runtime_context::Error::RegistrationChanged)
    ));
    assert_eq!(reader_a.target.as_ref().unwrap().registration_id(), REG);
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn foreign_permission_context_cannot_publish_an_agent_snapshot() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let context_path = format!("/api/agents/worker/runtime-context?expected_registration_id={REG}");
    let (_a, a, server) = provider(
        "a",
        vec![
            Reply::ok(&inspect_path, inspect("a")),
            Reply::ok(&context_path, permissions("b")),
        ],
    );
    assert!(matches!(
        reader(a).read(),
        Err(runtime_context::Error::Invalid)
    ));
    server.join().unwrap();
}
