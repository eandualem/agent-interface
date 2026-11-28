use super::*;
use crate::profiles::{
    access::{Failure, Grant, Role},
    browsers::{Create, End, Policies, Save, State},
};

fn text(name: &str) -> &'static str {
    match name {
        "browsers" => include_str!("../browsers/fixtures/browsers.json"),
        "browser" => include_str!("../browsers/fixtures/browser.json"),
        "browser-sessions" => include_str!("../browsers/fixtures/browser-sessions.json"),
        "browser-create" => include_str!("../browsers/fixtures/browser-create.json"),
        "browser-save" => include_str!("../browsers/fixtures/browser-save.json"),
        "browser-policies" => include_str!("../browsers/fixtures/browser-policies.json"),
        "agent-grant-browser" => include_str!("../browsers/fixtures/agent-grant-browser.json"),
        "access-request-browser" => {
            include_str!("../browsers/fixtures/access-request-browser.json")
        }
        "agent-access" => include_str!("../access/fixtures/agent-access.json"),
        "refusal-conflict" => include_str!("../access/fixtures/refusal-conflict.json"),
        _ => unreachable!("no fixture {name}"),
    }
}

fn fixture(name: &str) -> Value {
    serde_json::from_str(text(name)).unwrap()
}

/// A body as the fixture server records it, which reads requests in lower
/// case.
fn recorded(name: &str) -> Value {
    serde_json::from_str(&text(name).to_ascii_lowercase()).unwrap()
}

#[test]
fn the_browsers_their_detail_and_older_use_are_read() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, fixture("browsers")),
            (200, json!({"items":[{"id":"x"}],"wanted":[{"browser":1}]})),
            (200, fixture("browser")),
            (200, fixture("browser-sessions")),
            (200, fixture("browser-sessions")),
        ],
    );
    let c = profile.connect().unwrap();
    let list = c.browsers(None).unwrap();
    let states: Vec<_> = list.items.iter().map(|b| b.state).collect();
    assert_eq!(states, [State::InUse, State::Asked, State::SignedOut]);
    let instagram = &list.items[0];
    assert_eq!(instagram.machine_name.as_deref(), Some("laptop"));
    assert_eq!(instagram.holder.as_ref().unwrap().tabs, 3);
    assert_eq!(list.items[2].signed_in_since, None);
    assert_eq!(list.wanted[0].browser, "telegram");
    assert_eq!(list.wanted[0].requests[0].agent, "uploader");
    // An item that cannot be read is left out.
    let unread = c.browsers(Some("5f2c9e0d1b7a4e38")).unwrap();
    assert!(unread.items.is_empty() && unread.wanted.is_empty());

    let detail = c.browser("youtube").unwrap();
    let holder = detail.holder.as_ref().unwrap();
    assert_eq!(
        (holder.session, holder.agent_browser_session.as_deref()),
        (Some(41), Some("poster"))
    );
    assert!(
        detail
            .devtools
            .as_deref()
            .unwrap()
            .starts_with("ws://127.0.0.1:53121/")
    );
    assert_eq!(detail.grants[0].given_by, None);
    assert_eq!(detail.grants[0].uses, 14);
    assert_eq!(detail.givers[0].role, Role::Coordinator);
    assert_eq!(detail.requests[0].id, 17);
    assert!(detail.signed_in_to[0].note.is_some());
    assert_eq!(detail.signed_in_to[1].note, None);
    assert!(detail.policies.keep_on_sites && detail.policies.stop_before_last_button);
    let ends: Vec<_> = detail.recent.iter().map(|s| s.ended_by).collect();
    assert_eq!(ends, [None, Some(End::Released), Some(End::Closed)]);
    assert_eq!(detail.recent[2].agent, None);
    assert!(detail.has_more_recent);

    let older = c.browser_sessions("youtube", Some(40), None).unwrap();
    assert_eq!(older.items[1].ended_by, Some(End::TakenBack));
    assert_eq!((older.has_more, older.next_before), (false, None));
    c.browser_sessions("youtube", Some(40), Some(20)).unwrap();

    let requests = server.join().unwrap();
    let routes = [
        "get /api/browsers ",
        "get /api/browsers?machine=5f2c9e0d1b7a4e38 ",
        "get /api/browsers/youtube ",
        "get /api/browsers/youtube/sessions?before=40 ",
        "get /api/browsers/youtube/sessions?before=40&limit=20 ",
    ];
    for (request, route) in requests[1..].iter().zip(routes) {
        assert!(request.starts_with(route), "{request}");
    }
    assert_eq!(requests.len(), 1 + routes.len());
}

#[test]
fn each_change_uses_its_method_route_and_the_body_backbone_documents() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let detail = fixture("browser");
    let access = fixture("agent-access");
    let server = serve(
        listener,
        vec![
            (200, identity),
            (201, detail.clone()),
            (200, detail.clone()),
            (200, detail.clone()),
            (200, detail.clone()),
            (200, detail),
            (204, Value::Null),
            (204, Value::Null),
            (200, access.clone()),
            (200, access.clone()),
            (200, access),
        ],
    );
    let c = profile.connect().unwrap();
    let created = c
        .create_browser(&Create {
            name: "YouTube".into(),
            opens_at: "youtube.com".into(),
            machine: Some("laptop".into()),
        })
        .unwrap();
    assert_eq!(created.browser.id, "youtube");
    c.show_browser("youtube").unwrap();
    c.save_browser(
        "youtube",
        &Save {
            name: Some("YouTube".into()),
            opens_at: Some("youtube.com".into()),
            grants: vec!["uploader".into()],
            policies: Some(Policies {
                keep_on_sites: true,
                sites: vec!["youtube.com".into(), "studio.youtube.com".into()],
                stop_before_last_button: false,
            }),
        },
    )
    .unwrap();
    c.check_browser("youtube").unwrap();
    let policies: Policies = serde_json::from_value(fixture("browser-policies")).unwrap();
    c.set_browser_policies("youtube", &policies).unwrap();
    c.take_back_browser("youtube").unwrap();
    c.delete_browser("youtube").unwrap();
    // Who may use it and give it out are grants and ceilings naming it,
    // sent back as a reply has them.
    let Value::Object(mut body) = fixture("agent-grant-browser") else {
        unreachable!()
    };
    let browser: Grant = serde_json::from_value(body.remove("grant").unwrap()).unwrap();
    assert!(matches!(&browser, Grant::Browser { browser: Some(id), .. } if id == "youtube"));
    c.grant("uploader", &browser).unwrap();
    c.revoke("uploader", &browser).unwrap();
    c.set_ceiling("lead", std::slice::from_ref(&browser))
        .unwrap();

    let requests = server.join().unwrap();
    let grant = recorded("agent-grant-browser");
    let expected = [
        ("post /api/browsers ", Some(recorded("browser-create"))),
        ("post /api/browsers/youtube/window ", None),
        (
            "post /api/browsers/youtube/save ",
            Some(recorded("browser-save")),
        ),
        ("post /api/browsers/youtube/check ", None),
        (
            "put /api/browsers/youtube/policies ",
            Some(recorded("browser-policies")),
        ),
        ("delete /api/browsers/youtube/hold ", None),
        ("delete /api/browsers/youtube ", None),
        ("post /api/agents/uploader/grant ", Some(grant.clone())),
        ("post /api/agents/uploader/revoke ", Some(grant.clone())),
        (
            "put /api/agents/lead/ceiling ",
            Some(json!({ "ceiling": [grant["grant"]] })),
        ),
    ];
    let sent_count = expected.len();
    for (request, (route, body)) in requests[1..].iter().zip(expected) {
        assert!(request.starts_with(route), "{request}");
        let (_, sent) = request.split_once("\r\n\r\n").unwrap();
        match body {
            // Compared as JSON: key order depends on serde_json's features.
            Some(body) => assert_eq!(serde_json::from_str::<Value>(sent).unwrap(), body),
            None => assert_eq!(sent, "", "{request}"),
        }
    }
    assert_eq!(requests.len(), 1 + sent_count);
}

#[test]
fn a_refused_browser_change_keeps_backbones_code_and_message() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let server = serve(
        listener,
        vec![
            (200, identity),
            (409, fixture("refusal-conflict")),
            (409, fixture("refusal-conflict")),
        ],
    );
    let c = profile.connect().unwrap();
    let refused = Err(Failure::Refused {
        code: "conflict".into(),
        message: "Request 21 was already decided.".into(),
    });
    assert_eq!(
        c.save_browser("youtube", &Save::default()).map(drop),
        refused
    );
    assert_eq!(c.delete_browser("youtube"), refused);
    // A browser that cannot be named is never asked for.
    assert_eq!(
        c.browser("").map(drop),
        Err(Failure::Connection(Error::Invalid))
    );
    assert_eq!(server.join().unwrap().len(), 3);
}

#[test]
fn a_request_for_a_browser_names_it() {
    let request = fixture("access-request-browser");
    let grant: Grant = serde_json::from_value(request["grant"].clone()).unwrap();
    assert_eq!(
        grant,
        Grant::Browser {
            machine: Some("5f2c9e0d1b7a4e38".into()),
            machine_name: None,
            browser: Some("youtube".into()),
        }
    );
}
