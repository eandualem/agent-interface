use super::*;
use crate::roster::content::tests::{Reply, provider};
use agent_interface_core::AgentSection;
use serde_json::{Value, json};

const REG: &str = "1234567890abcdef1234567890abcdef";
fn inspect(label: &str) -> Value {
    json!({"name":"worker","registration_id":REG,"runtime":"codex","known":true,"repo":format!("sentinel-{label}")})
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
fn native_cards_retain_registration_and_delayed_profile_origin() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let (arrived, waiting) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let (_a, a, server_a) = provider(
        "a",
        vec![
            Reply {
                delay: Some((arrived, held)),
                ..Reply::ok(&inspect_path, inspect("a"))
            },
            Reply::ok(&inspect_path, inspect("a")),
            Reply {
                status: 409,
                ..Reply::ok(
                    &inspect_path,
                    json!({"detail":{"code":"registration_changed"}}),
                )
            },
        ],
    );
    let (_b, b, server_b) = provider("b", vec![Reply::ok(&inspect_path, inspect("b"))]);
    let worker = std::thread::spawn(move || {
        let mut reader = reader(a);
        let snapshot = reader.read(AgentSection::Overview).unwrap();
        (reader, snapshot)
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let snapshot_b = reader(b).read(AgentSection::Overview).unwrap();
    release.send(()).unwrap();
    let (mut reader_a, snapshot_a) = worker.join().unwrap();
    assert_eq!(snapshot_a.agent.repo, "sentinel-a");
    assert_eq!(snapshot_b.agent.repo, "sentinel-b");
    assert_eq!(
        reader_a.read(AgentSection::Overview).unwrap().agent.repo,
        "sentinel-a"
    );
    assert!(matches!(
        reader_a.read(AgentSection::Overview),
        Err(runtime_context::Error::RegistrationChanged)
    ));
    assert_eq!(reader_a.target.as_ref().unwrap().registration_id(), REG);
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn removal_refusals_say_what_happened() {
    assert!(removal_refusal(ProfileError::Refused(409)).contains("still running"));
    assert!(removal_refusal(ProfileError::Refused(403)).contains("refused"));
    assert!(removal_refusal(ProfileError::Unavailable).contains("connection"));
    assert!(removal_refusal(ProfileError::RegistrationChanged).contains("replaced"));
}

#[test]
fn removal_refuses_a_registration_replaced_after_the_check() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let stop_path = format!("/api/agents/worker/stop?expected_registration_id={REG}");
    let (_a, a, server) = provider(
        "a",
        vec![
            Reply::ok(&inspect_path, inspect("a")),
            Reply {
                status: 409,
                post: true,
                ..Reply::ok(
                    &stop_path,
                    json!({"detail":{"code":"registration_changed"}}),
                )
            },
        ],
    );
    assert_eq!(remove_checked(&a, "worker", REG), Err(REPLACED));
    server.join().unwrap();
}

#[test]
fn a_section_reads_on_the_card_connection_and_missing_parts_keep_the_card() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let (_a, a, server) = provider(
        "a",
        vec![
            Reply::ok(&inspect_path, inspect("a")),
            Reply {
                status: 503,
                ..Reply::ok("/api/skills/preview/worker", json!({}))
            },
        ],
    );
    let mut reader = reader(a);
    let snapshot = reader.read(AgentSection::Skills).unwrap();
    assert_eq!(snapshot.agent.repo, "sentinel-a");
    assert!(matches!(snapshot.agent.skills, Some(Err(_))));
    server.join().unwrap();
}

#[test]
fn removal_leaves_a_newer_agent_with_the_same_name_alone() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let (_a, a, server) = provider(
        "a",
        vec![Reply {
            status: 409,
            ..Reply::ok(
                &inspect_path,
                json!({"detail":{"code":"registration_changed"}}),
            )
        }],
    );
    assert_eq!(remove_checked(&a, "worker", REG), Err(REPLACED));
    server.join().unwrap();
}

#[test]
fn removing_an_agent_that_is_already_gone_succeeds() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let (_a, a, server) = provider(
        "a",
        vec![Reply {
            status: 404,
            ..Reply::ok(&inspect_path, json!({"detail":"not found"}))
        }],
    );
    assert_eq!(remove_checked(&a, "worker", REG), Ok(()));
    server.join().unwrap();
}

// The Rust service's preview names store skills without a folder or an
// identity; each row opens the skill the catalog lists under its name.
#[test]
fn skill_rows_take_their_catalog_identity_by_name() {
    let inspect_path = format!("/api/agents/worker/inspect?expected_registration_id={REG}");
    let preview = json!({"name":"worker","runtime":"codex","tags":["coder"],"directories":[],
        "skills":[
            {"name":"review","tags":["all"],"description":"r","links":{}},
            {"name":"notes","tags":["coder"],"description":"n","links":{}}
        ],
        "repository":[],"user_directories":[],"user":[],"notices":[]});
    let catalog = json!({"items":[
        {"name":"notes","description":"n","tags":["coder"],"error":null,"reaches":["worker"],"artifact_id":"artifact-2"},
        {"name":"review","description":"r","tags":["all"],"error":null,"reaches":["worker"],"artifact_id":"artifact-1"}
    ],"total":2});
    let (_a, a, server) = provider(
        "a",
        vec![
            Reply::ok(&inspect_path, inspect("a")),
            Reply::ok("/api/skills/preview/worker", preview),
            Reply::ok("/api/skills", catalog),
        ],
    );
    let snapshot = reader(a).read(AgentSection::Skills).unwrap();
    let Some(Ok(loadout)) = &snapshot.agent.skills else {
        panic!("skills were not read");
    };
    let rows: Vec<_> = loadout
        .skills
        .iter()
        .map(|skill| (skill.path.as_str(), skill.artifact_id.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [
            ("review", Some("artifact-1")),
            ("notes", Some("artifact-2"))
        ]
    );
    server.join().unwrap();
}

#[test]
fn the_overview_read_carries_the_wait_counts_and_processes_from_the_services_fixture() {
    let registration = "0193a1b2c3d47e5f8a9b0c1d2e3f4a5b";
    let inspected: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/src/profiles/runs/fixtures/agent-inspect.json"
    )))
    .unwrap();
    let inspect_path =
        format!("/api/agents/builder/inspect?expected_registration_id={registration}");
    let (_a, a, server) = provider("a", vec![Reply::ok(&inspect_path, inspected)]);
    let snapshot = Reader {
        connection: a,
        name: "builder".into(),
        registration: registration.into(),
        target: None,
    }
    .read(AgentSection::Overview)
    .unwrap();
    let agent = snapshot.agent;
    assert_eq!(agent.waiting, None);
    assert_eq!(agent.background.map(|counts| counts.running), Some(1));
    assert_eq!(agent.processes[0].command, "cargo test --workspace");
    server.join().unwrap();
}
