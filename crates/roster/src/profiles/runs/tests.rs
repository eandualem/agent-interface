use super::*;
use crate::profiles::secrets::tests::served;
use serde_json::{Value as Json, json};

/// The service's own fixtures, copied byte for byte.
fn fixture(name: &str) -> Json {
    serde_json::from_str(match name {
        "runs" => include_str!("fixtures/runs.json"),
        "run" => include_str!("fixtures/run.json"),
        _ => panic!("no fixture {name}"),
    })
    .unwrap()
}

/// A roster as the service lists it, with the fixture's agent counts.
fn roster(body: &[u8]) -> Snapshot {
    let mut snapshot = Snapshot::default();
    snapshot.update(body);
    snapshot
}

const REVIEW: &str = "0193a1b2c3d47e5f8a9b0c1d2e3f4a60";
const BUILD: &str = "0193a1b2c3d47e5f8a9b0c1d2e3f4a61";

#[test]
fn active_runs_and_one_run_decode_from_the_services_fixtures() {
    let (connection, server) = served(vec![(200, fixture("runs")), (200, fixture("run"))]);
    let runs = connection.runs().unwrap();
    assert_eq!(runs.len(), 2);
    let build = &runs[1];
    assert_eq!(
        (
            build.kind.as_str(),
            build.label.as_str(),
            build.state.as_str()
        ),
        ("build", "cargo build --workspace", "queued")
    );
    assert_eq!(build.waiting_on.as_deref(), Some("build slot"));
    assert_eq!(build.queued_at.as_deref(), Some("2026-11-10T10:13:05Z"));
    assert_eq!((&build.started_at, build.exit_code), (&None, None));
    let run = connection.run(REVIEW).unwrap();
    assert_eq!(run, runs[0]);
    assert_eq!(
        run.agent_id.as_deref(),
        Some("0193a1b2c3d47e5f8a9b0c1d2e3f4a5b")
    );
    assert_eq!(
        run.progress.as_deref(),
        Some("Reviewing crates/protocol/src/runs.rs")
    );
    assert_eq!(run.agent.as_deref(), Some("builder"));
    assert_eq!(
        run.review,
        Some(Review {
            runtime: Some("codex".into()),
            model: Some("gpt-example".into()),
            effort: Some("high".into()),
        })
    );
    assert_eq!(build.review, None);
    // Only a 32-digit lowercase ID is put in a path.
    for refused in [
        "../agents",
        "0193A1B2C3D47E5F8A9B0C1D2E3F4A60",
        "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a60",
    ] {
        assert_eq!(connection.run(refused), Err(Error::Invalid));
    }
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("GET /api/runs HTTP/1.1\r\n"));
    assert!(requests[2].starts_with(&format!("GET /api/runs/{REVIEW} HTTP/1.1\r\n")));
    assert_eq!(requests.len(), 3);
}

#[test]
fn a_run_that_left_the_list_is_kept_once_it_ended() {
    let mut ended = fixture("run");
    ended["state"] = json!("failed");
    ended["finished_at"] = json!("2026-11-10T10:21:47Z");
    ended["exit_code"] = json!(101);
    let build = fixture("runs")["items"][1].clone();
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        // The review ended; the build still waits.
        (200, json!({"items": [build]})),
        (200, ended),
        // The build ended too, but can no longer be read.
        (200, json!({"items": []})),
        (404, json!({"detail": "not found"})),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let idle = roster(br#"{"items":[{"name":"worker","state":"idle","background":{"running":0,"queued":0,"processes":0}}]}"#);
    let mut book = Book::default();
    book.follow(&connection, &busy);
    assert_eq!(book.runs().active, decoded(fixture("runs")));
    assert!(book.runs().finished.is_empty());
    book.follow(&connection, &busy);
    let ids = |runs: &[Run]| {
        runs.iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&book.runs().active), [BUILD]);
    assert_eq!(book.runs().finished.len(), 1);
    assert_eq!(book.runs().finished[0].exit_code, Some(101));
    // No agent reports runs, but the book still held one, so it asks once more.
    book.follow(&connection, &idle);
    assert!(book.runs().active.is_empty());
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    // Nothing is active and nobody reports runs: nothing is asked.
    book.follow(&connection, &idle);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests[3].starts_with(&format!("GET /api/runs/{REVIEW} HTTP/1.1\r\n")));
    assert!(requests[5].starts_with(&format!("GET /api/runs/{BUILD} HTTP/1.1\r\n")));
    // Each agent sees only its own runs.
    let worker = book.runs().of("0193a1b2c3d47e5f8a9b0c1d2e3f4a5b");
    assert_eq!(worker.finished.len(), 1);
    assert_eq!(
        book.runs().of("1234567890abcdef1234567890abcdef"),
        Runs::default()
    );
}

/// A list's runs as they decode.
fn decoded(listed: Json) -> Vec<Run> {
    serde_json::from_str(&listed["items"].to_string()).unwrap()
}

#[test]
fn an_older_service_is_never_asked_and_one_without_runs_is_asked_once() {
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        (503, json!({})),
        (404, json!({"detail": "Not Found"})),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    // A profile the Rust service doesn't serve is never asked.
    let mut earlier = connection.clone();
    earlier.served = false;
    assert_eq!(earlier.runs(), Err(Error::Unsupported));
    assert_eq!(earlier.run(REVIEW), Err(Error::Unsupported));
    Book::default().follow(&earlier, &busy);
    let mut book = Book::default();
    // An older service lists agents without counts.
    book.follow(
        &connection,
        &roster(br#"{"items":[{"name":"worker","state":"busy"}]}"#),
    );
    book.follow(&connection, &busy);
    assert_eq!(book.runs().active.len(), 2);
    // A failed read keeps the book.
    book.follow(&connection, &busy);
    assert_eq!(book.runs().active.len(), 2);
    // A service without the route has nothing running and isn't asked again.
    book.follow(&connection, &busy);
    book.follow(&connection, &busy);
    assert_eq!(book.runs(), &Runs::default());
    assert_eq!(server.join().unwrap().len(), 4);
}

#[test]
fn a_run_that_is_not_the_one_asked_for_or_does_not_decode_is_refused() {
    let mut other = fixture("run");
    other["run_id"] = json!(BUILD);
    // Mistyped optional fields read as absent and keep the run.
    let mut mistyped = fixture("run");
    for (field, value) in [
        ("exit_code", json!("zero")),
        ("progress", json!(7)),
        ("agent", json!(["builder"])),
    ] {
        mistyped[field] = value;
    }
    mistyped["review"]["model"] = json!(5);
    let (connection, server) = served(vec![
        (200, other),
        (
            200,
            json!({"items": [{"run_id": REVIEW}, mistyped, {"kind": "test"}]}),
        ),
        (200, json!({"items": {}})),
    ]);
    assert_eq!(connection.run(REVIEW), Err(Error::Invalid));
    let listed = connection.runs().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].exit_code, &listed[0].progress), (None, &None));
    assert_eq!(listed[0].agent, None);
    let review = listed[0].review.as_ref().unwrap();
    assert_eq!(
        (review.model.as_deref(), review.runtime.as_deref()),
        (None, Some("codex"))
    );
    assert_eq!(connection.runs(), Err(Error::Invalid));
    server.join().unwrap();
}
