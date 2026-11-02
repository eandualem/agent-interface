use super::*;
use crate::roster::content::tests::{Reply, provider, record};
use agent_interface_roster::conversations::{Kind, Page};
use serde_json::{Value, json};
use std::sync::mpsc;

fn fixture(name: &str) -> Value {
    let bytes: &[u8] = match name {
        "summary" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/tests/fixtures/interactions/summary.json"
        )),
        "history" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/tests/fixtures/interactions/history.json"
        )),
        "detail" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/tests/fixtures/interactions/detail.json"
        )),
        "states" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/tests/fixtures/interactions/state-spans.json"
        )),
        _ => unreachable!(),
    };
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn bound_interaction_navigation_and_discussions_keep_the_snapshot_origin() {
    let summary = "/api/interactions/summary?since=start&until=end";
    let history = "/api/interactions?since=start&until=end&agent_id=worker&direction=both&limit=200&snapshot=fixture-not-a-live-cursor";
    let states = "/api/interactions/state-spans?since=start&until=end&limit=500&agent_id=worker";
    let id = "11111111111111111111111111111111";
    let chat = |kind: &str, id: &str| json!({"kind":kind,"artifact_id":id,"responsible":{"agent_id":"worker-id","name":"worker","status":"active"},"total":0,"unread":0,"entries":[],"older":null});
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let mut detail = fixture("detail");
    detail["body"]["text"] = json!("sentinel-a");
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Reply {
                delay: Some((arrived, delayed)),
                ..Reply::ok(summary, fixture("summary"))
            },
            Reply::ok(history, fixture("history")),
            Reply::ok(states, fixture("states")),
            Reply::ok("/api/interactions/11111111111111111111111111111111", detail),
            Reply::ok("/api/responses/7", record("a-link")),
            Reply::ok(
                "/api/conversations/interaction/11111111111111111111111111111111?limit=50",
                chat("interaction", id),
            ),
            Reply::ok(
                "/api/conversations/document/7?limit=50",
                chat("document", "7"),
            ),
            Reply::ok(summary, fixture("summary")),
            Reply::ok(history, fixture("history")),
            Reply {
                status: 409,
                replace: true,
                ..Reply::ok(states, json!({"detail":{"code":"binding_mismatch"}}))
            },
        ],
    );
    let (_dir_b, b, server_b) = provider(
        "b",
        vec![
            Reply::ok(summary, fixture("summary")),
            Reply::ok(history, fixture("history")),
            Reply::ok(states, fixture("states")),
        ],
    );
    let source_a = Source::Bound(a.clone());
    let source_b = Source::Bound(b);
    assert!(!source_a.belongs_to(&crate::desktop::agent_status::LiveAgents::default()));
    let worker = std::thread::spawn({
        let source = source_a.clone();
        move || source.read("start", "end", Some("worker"))
    });
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    let snapshot_b = source_b.read("start", "end", Some("worker")).unwrap();
    assert!(!source_a.accepts(&snapshot_b));
    assert!(matches!(
        source_a.detail(&snapshot_b, id),
        Err(Error::Connection(
            agent_interface_roster::profiles::Error::BindingMismatch
        ))
    ));
    std::fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    release.send(()).unwrap();
    let snapshot_a = worker.join().unwrap().unwrap();
    assert!(source_a.accepts(&snapshot_a));
    assert!(source_a.directory().is_none());
    assert_ne!(
        source_a.key(&snapshot_a.provider),
        source_b.key(&snapshot_b.provider)
    );
    assert_eq!(
        source_a
            .detail(&snapshot_a, id)
            .unwrap()
            .body
            .text
            .as_deref(),
        Some("sentinel-a")
    );
    assert!(source_a.report(&source_b.key(""), 7, true).is_err());
    assert!(
        format!("{:?}", source_a.report(&source_a.key(""), 7, true).unwrap())
            .contains("sentinel-a-link")
    );
    for (kind, id) in [(Kind::Interaction, id), (Kind::Document, "7")] {
        let target = super::super::conversation::ReferenceTarget {
            provider: String::new(),
            connection: source_a.connection().cloned(),
            kind,
            id: id.into(),
        };
        let client = target.bound_client().unwrap().unwrap();
        assert_eq!(client.identity(), a.identity());
        assert!(client.show(&Page::Newest).unwrap().entries.is_empty());
    }
    assert!(matches!(
        source_a.read("start", "end", Some("worker")),
        Err(Error::Connection(
            agent_interface_roster::profiles::Error::BindingMismatch
        ))
    ));
    server_a.join().unwrap();
    server_b.join().unwrap();
}
