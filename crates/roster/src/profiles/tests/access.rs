use super::*;
use crate::profiles::access::{
    Access, Failure, Grant, Mode, Reach, Reached, RequestState, Requests, Role, Source,
};

fn fixture(name: &str) -> Value {
    let text = match name {
        "agents" => include_str!("../access/fixtures/agents.json"),
        "swarms" => include_str!("../access/fixtures/swarms.json"),
        "requests-pending" => include_str!("../access/fixtures/requests-pending.json"),
        "requests-all" => include_str!("../access/fixtures/requests-all.json"),
        "agent-access" => include_str!("../access/fixtures/agent-access.json"),
        "agent-placed" => include_str!("../access/fixtures/agent-placed.json"),
        "request-approved" => include_str!("../access/fixtures/request-approved.json"),
        "refusal-outside-authority" => {
            include_str!("../access/fixtures/refusal-outside-authority.json")
        }
        "refusal-not-found" => include_str!("../access/fixtures/refusal-not-found.json"),
        "refusal-conflict" => include_str!("../access/fixtures/refusal-conflict.json"),
        "refusal-invalid" => include_str!("../access/fixtures/refusal-invalid.json"),
        _ => unreachable!("no fixture {name}"),
    };
    serde_json::from_str(text).unwrap()
}

#[test]
fn the_overview_lists_registered_agents_with_their_place_access_and_swarms() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, fixture("agents")),
            (200, fixture("swarms")),
        ],
    );
    let overview = profile.connect().unwrap().access_overview().unwrap();
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("get /api/agents "));
    assert!(requests[2].starts_with("get /api/swarms "));
    // An unregistered session and one Backbone does not manage are left out.
    let names: Vec<_> = overview.agents.iter().map(|a| a.name.as_str()).collect();
    assert!(!names.contains(&"scratch") && !names.contains(&"stray"));
    assert_eq!(names.len(), 10);
    let lead = &overview.agents[0];
    assert_eq!(
        (lead.role, lead.superior.as_deref()),
        (Some(Role::Orchestrator), None)
    );
    assert_eq!(
        lead.reach,
        Reach::Profile {
            items: vec![Reached {
                repo: "example/platform".into(),
                access: Access::Write,
                source: Source::Grant,
            }]
        }
    );
    assert_eq!(lead.ceiling.len(), 2);
    let site = overview
        .agents
        .iter()
        .find(|a| a.name == "docs-site")
        .unwrap();
    assert_eq!(site.mode, Mode::Unsandboxed);
    assert_eq!(site.superior.as_deref(), Some("outreach"));
    assert!(matches!(
        &site.grants[..],
        [
            Grant::Folder { access: Access::Write, .. },
            Grant::SecretVariable { name, .. },
            Grant::SecretFile { machine_name: None, .. },
            Grant::Browser { .. },
            Grant::Docker { .. },
            // A kind this version does not know.
            Grant::Unknown,
        ] if name == "SITE_API_KEY"
    ));
    let member = overview
        .agents
        .iter()
        .find(|a| a.name == "app-squad-coder")
        .unwrap();
    assert_eq!(member.role, Some(Role::SwarmMember));
    assert_eq!(overview.swarms[0].name, "app-squad");
    assert_eq!(overview.swarms[0].initiator, "builder");
    assert_eq!(overview.swarms[0].members.len(), 3);
}

#[test]
fn requests_read_waiting_or_every_page_in_order() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, fixture("requests-pending")),
            (200, fixture("requests-all")),
            (
                200,
                json!({"items":[{"id":"not a number"}],"has_more":false}),
            ),
        ],
    );
    let c = profile.connect().unwrap();
    let pending = c.access_requests(Requests::Pending).unwrap();
    assert_eq!(pending.items.len(), 3);
    assert!(!pending.has_more);
    let first = &pending.items[0];
    assert_eq!((first.id, first.agent.as_str()), (21, "tester"));
    assert_eq!(first.state, RequestState::Pending);
    assert!(first.warning.is_some());
    assert_eq!(pending.items[1].waits_on.as_deref(), Some("builder"));
    let all = c.access_requests(Requests::All { before: None }).unwrap();
    assert_eq!((all.has_more, all.next_before_id), (true, Some(17)));
    let refused = &all.items[4];
    assert_eq!(refused.state, RequestState::Refused);
    assert_eq!(refused.decided_by, None);
    assert_eq!(refused.note, "Use the hosted preview instead.");
    assert_eq!(all.items[3].decided_by.as_deref(), Some("planner"));
    // A request that cannot be read is left out of its page.
    let older = c
        .access_requests(Requests::All { before: Some(17) })
        .unwrap();
    assert!(older.items.is_empty());
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("get /api/access/requests?state=pending "));
    assert!(requests[2].starts_with("get /api/access/requests?state=all "));
    assert!(requests[3].starts_with("get /api/access/requests?state=all&before_id=17 "));
}

#[test]
fn each_change_uses_its_method_route_and_body() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let access = fixture("agent-access");
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, access.clone()),
            (200, access.clone()),
            (200, access),
            (200, fixture("agent-placed")),
            (200, fixture("agent-placed")),
            (200, fixture("request-approved")),
            (200, fixture("request-approved")),
        ],
    );
    let c = profile.connect().unwrap();
    let browser: Grant = serde_json::from_value(
        json!({"kind":"browser","machine":"5f2c9e0d1b7a4e38","machine_name":"laptop"}),
    )
    .unwrap();
    let granted = c.grant("tester", &browser).unwrap();
    assert_eq!(granted.grants, std::slice::from_ref(&browser));
    assert!(granted.warning.is_some());
    c.revoke("tester", &browser).unwrap();
    c.set_mode("tester", Mode::Unsandboxed).unwrap();
    let placed = c.place("tester", Role::Coordinator, Some("lead")).unwrap();
    assert_eq!(
        (placed.role, placed.superior.as_deref()),
        (Role::Coordinator, Some("lead"))
    );
    c.place("tester", Role::Main, None).unwrap();
    let approved = c.approve(21, "").unwrap();
    assert_eq!(approved.state, RequestState::Approved);
    c.refuse(21, "Not needed").unwrap();
    let requests = server.join().unwrap();
    let expected = [
        (
            "post /api/agents/tester/grant ",
            r#"{"grant":{"kind":"browser","machine":"5f2c9e0d1b7a4e38","machine_name":"laptop"}}"#,
        ),
        (
            "post /api/agents/tester/revoke ",
            r#"{"grant":{"kind":"browser","machine":"5f2c9e0d1b7a4e38","machine_name":"laptop"}}"#,
        ),
        ("put /api/agents/tester/mode ", r#"{"mode":"unsandboxed"}"#),
        (
            "put /api/agents/tester/place ",
            r#"{"role":"coordinator","superior":"lead"}"#,
        ),
        (
            "put /api/agents/tester/place ",
            r#"{"role":"main","superior":null}"#,
        ),
        ("post /api/access/requests/21/approve ", r#"{"note":""}"#),
        // The fixture server reads requests in lower case.
        (
            "post /api/access/requests/21/refuse ",
            r#"{"note":"not needed"}"#,
        ),
    ];
    for (request, (route, body)) in requests[1..].iter().zip(expected) {
        assert!(request.starts_with(route), "{request}");
        // Compared as JSON: key order depends on serde_json's features.
        let (_, sent) = request.split_once("\r\n\r\n").unwrap();
        let sent: Value = serde_json::from_str(sent).unwrap();
        assert_eq!(
            sent,
            serde_json::from_str::<Value>(body).unwrap(),
            "{request}"
        );
        assert!(!request.contains("x-backbone-request-id"));
    }
    assert_eq!(requests.len(), 1 + expected.len());
}

#[test]
fn a_refused_change_keeps_backbones_code_and_message() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let server = serve(
        listener,
        vec![
            (200, identity),
            (403, fixture("refusal-outside-authority")),
            (404, fixture("refusal-not-found")),
            (409, fixture("refusal-conflict")),
            (422, fixture("refusal-invalid")),
            (500, json!({})),
        ],
    );
    let c = profile.connect().unwrap();
    let refused = |code: &str, message: &str| {
        Err(Failure::Refused {
            code: code.into(),
            message: message.into(),
        })
    };
    assert_eq!(
        c.set_mode("tester", Mode::Unsandboxed).map(drop),
        refused(
            "outside_authority",
            "Only the owner may set how an agent is confined."
        )
    );
    assert_eq!(
        c.grant(
            "tester",
            &Grant::Docker {
                machine: None,
                machine_name: None
            }
        )
        .map(drop),
        refused(
            "not_found",
            "No agent named 'tester' is registered in this profile."
        )
    );
    assert_eq!(
        c.approve(21, "").map(drop),
        refused("conflict", "Request 21 was already decided.")
    );
    let folder = Grant::Folder {
        machine: None,
        machine_name: None,
        path: "data".into(),
        access: Access::Read,
    };
    assert_eq!(
        c.grant("tester", &folder).map(drop),
        refused("invalid_request", "path: use an absolute path.")
    );
    // Without a reason the status is kept.
    assert_eq!(
        c.refuse(21, "").map(drop),
        Err(Failure::Connection(Error::Refused(500)))
    );
    server.join().unwrap();
}

#[test]
fn a_change_that_cannot_be_named_is_never_sent() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let server = serve(listener, vec![(200, identity)]);
    let c = profile.connect().unwrap();
    let invalid = Err(Failure::Connection(Error::Invalid));
    assert_eq!(c.revoke("tester", &Grant::Unknown).map(drop), invalid);
    assert_eq!(c.set_mode("tester", Mode::Unknown).map(drop), invalid);
    assert_eq!(c.place("tester", Role::Unknown, None).map(drop), invalid);
    assert_eq!(
        c.place("tester", Role::Main, Some("../x")).map(drop),
        invalid
    );
    assert_eq!(c.set_mode("../x", Mode::Sandboxed).map(drop), invalid);
    assert_eq!(server.join().unwrap().len(), 1);
}
