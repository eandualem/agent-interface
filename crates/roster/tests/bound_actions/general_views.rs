use super::*;
use agent_interface_roster::views::{
    Data, Range,
    bound::{self, Request},
};

fn agents(label: &str) -> Value {
    json!({"items":[{"name":"worker","repo":label,"runtime":"claude","tags":["coder"]}]})
}

fn prefix(path: &'static str, reply: Value) -> Step {
    Step {
        prefix: true,
        ..Step::get(path, reply)
    }
}

#[test]
fn general_view_chains_keep_original_identity_and_credentials() {
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_dir_a, a, server_a) = provider(
        "a",
        vec![
            Step {
                delay: Some((arrived, delayed)),
                ..Step::get("/api/agents", agents("sentinel-a"))
            },
            Step::get("/api/swarms", json!({"items":[]})),
            Step::get("/api/skills", json!({"items":[]})),
            Step::get("/api/agents", agents("sentinel-a")),
            Step::get(
                "/api/skills/preview/worker",
                json!({"notices":["sentinel-a"]}),
            ),
            Step::get("/api/agents", agents("sentinel-a")),
            Step::get(
                "/api/agents/worker/instructions",
                json!({"name":"worker","sources":[]}),
            ),
            Step::get("/api/agents", agents("sentinel-a")),
            Step::get("/api/skills", json!({"items":[]})),
            Step::get(
                "/api/config",
                json!({"agents.shared_policy":{"value":["sentinel-a"]}}),
            ),
            prefix(
                "/api/usage?refresh=false&limit=1000&since=",
                json!({"items":[],"totals":{"total_tokens":37}}),
            ),
            Step {
                reply: Some((404, json!({}))),
                ..prefix("/api/agents/states?since=", json!({}))
            },
            Step::get("/api/deliveries?limit=200", json!({"items":[]})),
            Step::get("/api/events?limit=200", json!({"items":[]})),
            Step::get("/api/agents", agents("sentinel-a")),
        ],
    );
    let (_dir_b, b, server_b) = provider(
        "b",
        vec![
            Step::get("/api/agents", agents("sentinel-b")),
            Step::get("/api/swarms", json!({"items":[]})),
        ],
    );
    let worker = thread::spawn({
        let a = a.clone();
        move || bound::read(&a, &Request::Team, &|| true)
    });
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    let read_b = bound::read(&b, &Request::Team, &|| true);
    assert_eq!(read_b.identity, *b.identity());
    assert!(format!("{:?}", read_b.result).contains("sentinel-b"));
    fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    release.send(()).unwrap();
    let read_a = worker.join().unwrap();
    assert_eq!(read_a.identity, *a.identity());
    assert_eq!(read_a.request, Request::Team);
    assert!(format!("{:?}", read_a.result).contains("sentinel-a"));
    for request in [
        Request::Skills,
        Request::Policies,
        Request::Tags,
        Request::Usage(Range::Day),
        Request::Activity,
    ] {
        let read = bound::read(&a, &request, &|| true);
        assert_eq!(read.identity, *a.identity());
        assert_eq!(read.request, request);
        let data = read.result.unwrap();
        match (request, data) {
            (Request::Skills, Data::Skills(library)) => assert_eq!(library.previewed, 1),
            (Request::Policies, Data::Policies(library)) => assert_eq!(library.read, 1),
            (Request::Tags, Data::Tags(library)) => assert_eq!(library.global, ["sentinel-a"]),
            (Request::Usage(_), Data::Usage(usage)) => assert_eq!(usage.totals.total, 37),
            (Request::Activity, Data::Activity(activity)) => {
                assert_eq!(activity.members[0].repo, "sentinel-a")
            }
            other => panic!("unexpected read: {other:?}"),
        }
    }
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn optional_enrichment_binding_refusals_discard_the_whole_general_view() {
    let roster = json!({"items":[{"name":"worker"},{"name":"scout"},{"name":"later"}]});
    for request in [Request::Skills, Request::Policies, Request::Activity] {
        let mut steps = match request {
            Request::Skills => vec![
                Step::get("/api/skills", json!({"items":[]})),
                Step::get("/api/agents", roster.clone()),
                Step::get("/api/skills/preview/worker", json!({})),
                Step::get("/api/skills/preview/scout", json!({})),
            ],
            Request::Policies => vec![
                Step::get("/api/agents", roster.clone()),
                Step::get("/api/agents/worker/instructions", json!({"sources":[]})),
                Step::get("/api/agents/scout/instructions", json!({"sources":[]})),
            ],
            Request::Activity => vec![prefix("/api/agents/states?since=", json!({}))],
            _ => unreachable!(),
        };
        steps.last_mut().unwrap().reply =
            Some((409, json!({"detail":{"code":"binding_mismatch"}})));
        let (_dir, connection, server) = provider("a", steps);
        let read = bound::read(&connection, &request, &|| true);
        assert_eq!(
            read.result,
            Err(bound::Error::Connection(Error::BindingMismatch))
        );
        server.join().unwrap();
    }
}

#[test]
fn cancelled_general_view_stops_before_the_next_get() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let (arrived, reading) = mpsc::channel();
    let (release, delayed) = mpsc::channel();
    let (_dir, connection, server) = provider(
        "a",
        vec![Step {
            delay: Some((arrived, delayed)),
            ..Step::get("/api/agents", agents("a"))
        }],
    );
    let live = Arc::new(AtomicBool::new(true));
    let worker = thread::spawn({
        let live = live.clone();
        move || {
            bound::read(&connection, &Request::Team, &|| {
                live.load(Ordering::Acquire)
            })
        }
    });
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    live.store(false, Ordering::Release);
    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap().result, Err(bound::Error::Cancelled));
    server.join().unwrap();
}
