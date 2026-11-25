use super::*;
use crate::profiles::peers::{Decision, Direction, Policy};

#[test]
fn owner_routes_use_the_profile_socket_generation_and_original_request_key() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let generation = json!({"generation":"gen","peer_id":"peer"});
    let peer = json!({"connection":generation,"state":"connected","gates":{"incoming":"review","outgoing":"auto"}});
    let invitation = json!({"request_id":"req","peer_id":"peer","direction":"incoming","expires_at":"later","state":"pending"});
    let mutable = json!({"connection":generation,"message_id":"draft","direction":"outgoing","text":"line 1\nline 2","submitted_at":"now","expires_at":"later","state":"pending_review","receipt":{"kind":"missing"}});
    let history = json!({"items":[mutable],"older":"previous+page/="});
    let overview = json!({"local_id":"owner","connections":[peer],"invitations":[invitation],"pending_reviews":1});
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, overview),
            (200, invitation.clone()),
            (200, peer.clone()),
            (200, history),
            (
                200,
                json!({"items":[{"message":mutable,"queued_at":"now"}],"older":null}),
            ),
            (200, invitation.clone()),
            (200, json!({"invitation":invitation,"connection":peer})),
            (200, peer.clone()),
            (503, json!({})),
            (200, mutable.clone()),
            (200, peer),
            (
                200,
                json!({"items":[{ "connection":{"generation":"other","peer_id":"peer"}, "message_id":"foreign", "direction":"incoming", "text":"foreign","submitted_at":"t","expires_at":"t","state":"queued","receipt":{"kind":"missing"}}],"older":null}),
            ),
        ],
    );
    let c = profile.connect().unwrap();
    let data = c.peer_overview().unwrap();
    assert_eq!(data.local_id, "owner");
    assert_eq!(c.peer_invitation("req").unwrap().request_id, "req");
    let identity = c.peer_connection("gen").unwrap().connection;
    let page = c.peer_history(&identity, Some("previous+page/=")).unwrap();
    assert_eq!(page.items[0].text, "line 1\nline 2");
    let mut review = c.peer_reviews(None).unwrap().items.remove(0).message;
    c.invite_peer("peer", "invite-key").unwrap();
    c.answer_peer("req", true, "accept-key").unwrap();
    c.set_peer_gate("gen", Direction::Incoming, Policy::Auto, "gate-key")
        .unwrap();
    assert!(
        c.review_peer_message(&review, Decision::Revise("change"), "decision-key")
            .is_err()
    );
    c.review_peer_message(&review, Decision::Revise("change"), "decision-key")
        .unwrap();
    review.direction = Direction::Incoming;
    assert!(matches!(
        c.review_peer_message(&review, Decision::Revise("change"), "other-key"),
        Err(AddError::Connection(Error::Invalid))
    ));
    c.disconnect_peer("gen", "disconnect-key").unwrap();
    assert!(matches!(
        c.peer_history(&identity, None),
        Err(Error::BindingMismatch)
    ));
    let requests = server.join().unwrap();
    for request in &requests {
        assert!(request.contains("x-backbone-profile-id: "));
    }
    let routes = [
        "get /api/profile ",
        "get /api/peer-connections ",
        "get /api/peer-connections/requests/req ",
        "get /api/peer-connections/gen ",
        "get /api/peer-connections/gen/history?before=previous%2bpage%2f%3d ",
        "get /api/peer-connections/reviews ",
        "post /api/peer-connections/requests ",
        "post /api/peer-connections/requests/req/answer ",
        "patch /api/peer-connections/gen/gates ",
        "post /api/peer-connections/gen/messages/draft/review ",
        "post /api/peer-connections/gen/messages/draft/review ",
        "post /api/peer-connections/gen/disconnect ",
        "get /api/peer-connections/gen/history ",
    ];
    assert_eq!(requests.len(), routes.len());
    for (wire, route) in requests.iter().zip(routes) {
        assert!(wire.starts_with(route), "{route}");
    }
    assert_eq!(requests[9], requests[10]);
    for (request, (key, body)) in requests[6..12].iter().zip([
        ("invite-key", json!({"peer_id":"peer"})),
        ("accept-key", json!({"action":"accept"})),
        ("gate-key", json!({"direction":"incoming","policy":"auto"})),
        (
            "decision-key",
            json!({"action":"ask_to_revise","note":"change"}),
        ),
        (
            "decision-key",
            json!({"action":"ask_to_revise","note":"change"}),
        ),
        ("disconnect-key", json!({})),
    ]) {
        assert!(request.contains(&format!("x-backbone-request-id: {key}\r\n")));
        assert_eq!(
            serde_json::from_str::<Value>(request.split_once("\r\n\r\n").unwrap().1).unwrap(),
            body
        );
    }
}

#[test]
fn reconnect_supplies_last_event_id_before_refreshing_the_owner_snapshot() {
    let (directory, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(directory.path(), &socket);
    let handshake = serve(listener.try_clone().unwrap(), vec![(200, identity)]);
    let c = profile.connect().unwrap();
    handshake.join().unwrap();
    listener.set_nonblocking(false).unwrap();
    let server = thread::spawn(move || {
        for (i,frame) in ["event: hello\ndata: {\"cursor\":\"run:3\"}\n\nid: run:4\nevent: peer.connections.changed\ndata: {}\n\n","event: hello\ndata: {\"cursor\":\"run:6\"}\n\nid: run:6\nevent: resync\ndata: {}\n\n"].into_iter().enumerate(){
            let (mut stream,_)=listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
            let mut bytes=Vec::new();
            while !bytes.ends_with(b"\r\n\r\n"){
                let mut byte=[0];stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);assert!(bytes.len()<8192);
            }
            let headers=String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            assert!(headers.starts_with("get /api/stream?kinds=peer.connections.changed,resync "));
            assert_eq!(headers.contains("last-event-id: run:4\r\n"),i==1);
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{frame}",frame.len()).unwrap();
        }
    });
    let mut cursor = None;
    let mut changes = 0;
    for expected in ["run:4", "run:6"] {
        let _ = c.watch_peers(&mut cursor, &|| true, &mut || changes += 1);
        assert_eq!(cursor.as_deref(), Some(expected));
    }
    assert_eq!(changes, 4);
    server.join().unwrap();
}

#[test]
fn a_refused_connection_request_keeps_the_service_reason() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let refusal = |code, message| json!({"detail":{"code":code,"message":message}});
    let no_relay = "no relay is set: set peer_connections.relay_url";
    let own_id = "this profile's own connection ID";
    let lapsed = "this request cannot be answered: it was sent from here, answered, or lapsed";
    let server = serve(
        listener,
        vec![
            (200, identity),
            (409, refusal("conflict", no_relay)),
            (422, refusal("invalid_request", own_id)),
            (409, refusal("conflict", lapsed)),
            (409, json!({})),
            (404, refusal("not_found", "unknown peer invitation")),
            (
                409,
                refusal("binding_required", "bind this workspace first"),
            ),
        ],
    );
    let c = profile.connect().unwrap();
    let reason = |text: &str| Err(AddError::Refused(text.into()));
    assert_eq!(c.invite_peer("peer", "k1").map(drop), reason(no_relay));
    assert_eq!(c.invite_peer("peer", "k2").map(drop), reason(own_id));
    assert_eq!(c.answer_peer("req", true, "k3").map(drop), reason(lapsed));
    let status = |status| Err(AddError::Connection(Error::Refused(status)));
    assert_eq!(c.answer_peer("req", false, "k4").map(drop), status(409));
    assert_eq!(c.answer_peer("req", true, "k5").map(drop), status(404));
    // A binding refusal is never shown as a reason.
    assert_eq!(
        c.invite_peer("peer", "k6").map(drop),
        Err(AddError::Connection(Error::BindingRequired))
    );
    server.join().unwrap();
}

#[test]
fn a_refused_peer_change_keeps_the_service_reason() {
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let gone = "no active peer connection for this generation";
    let reviewed = "this message has already been reviewed";
    let refusal = |code, message| json!({"detail":{"code":code,"message":message}});
    let server = serve(
        listener,
        vec![
            (200, identity),
            (404, refusal("not_found", gone)),
            (409, refusal("conflict", reviewed)),
            (404, refusal("not_found", gone)),
            (409, json!({})),
            (503, refusal("unavailable", "the service is shutting down")),
        ],
    );
    let c = profile.connect().unwrap();
    let message: crate::profiles::peers::Message = serde_json::from_value(json!({"connection":{"generation":"gen","peer_id":"peer"},"message_id":"m","direction":"incoming","text":"t","submitted_at":"now","expires_at":"later","state":"pending_review","receipt":{"kind":"missing"}})).unwrap();
    let reason = |text: &str| Err(AddError::Refused(text.into()));
    assert_eq!(
        c.set_peer_gate("gen", Direction::Incoming, Policy::Auto, "k1")
            .map(drop),
        reason(gone)
    );
    assert_eq!(
        c.review_peer_message(&message, Decision::Pass, "k2")
            .map(drop),
        reason(reviewed)
    );
    assert_eq!(c.disconnect_peer("gen", "k3").map(drop), reason(gone));
    // Without a reason the status is kept.
    assert_eq!(
        c.disconnect_peer("gen", "k4").map(drop),
        Err(AddError::Connection(Error::Refused(409)))
    );
    // A server error keeps its status, so the app retries the same request.
    assert_eq!(
        c.disconnect_peer("gen", "k5").map(drop),
        Err(AddError::Connection(Error::Refused(503)))
    );
    server.join().unwrap();
}

#[test]
fn a_new_policy_only_affects_its_own_direction() {
    let data: super::super::peers::Gates =
        serde_json::from_value(json!({"outgoing":"future","incoming":"review"})).unwrap();
    assert_eq!(data.incoming, Policy::Review);
    assert_eq!(data.outgoing, Policy::Unknown);
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../peers/fixtures/", $name, ".json"))
    };
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> T {
    serde_json::from_str(text).unwrap()
}

#[test]
fn service_fixtures_parse_into_the_client_shapes() {
    use super::super::peers::{Answer, Invitation, Message, Overview, Page, Peer, Receipt, Review};
    let overview: Overview = parse(fixture!("peer-overview"));
    assert_eq!(overview.pending_reviews, 2);
    assert_eq!(overview.invitations[1].state, "declined");
    assert_eq!(overview.connections[0].gates.outgoing, Policy::Review);
    assert_eq!(
        overview.invitations[0].display_name.as_deref(),
        Some("Second peer")
    );
    assert_eq!(
        overview.connections[0].display_name.as_deref(),
        Some("Example peer")
    );
    let answer: Answer = parse(fixture!("peer-invitation-accepted"));
    assert_eq!(
        answer.invitation.generation.as_deref(),
        Some(answer.connection.unwrap().connection.generation.as_str())
    );
    let history: Page<Message> = parse(fixture!("peer-history"));
    assert_eq!(history.older.as_deref(), Some("p1-example-cursor"));
    assert!(matches!(history.items[1].receipt, Receipt::Uncertain));
    assert!(
        matches!(&history.items[3].receipt, Receipt::Confirmed { outcome, .. } if outcome == "declined")
    );
    let older: Page<Message> = parse(fixture!("peer-history-older"));
    assert!(older.older.is_none());
    let refused = older
        .items
        .iter()
        .find(|m| m.state == "upload_failed")
        .unwrap();
    let failure = refused.upload_failure.as_ref().unwrap();
    assert_eq!(failure.code, "mailbox_full");
    assert_eq!(failure.retry_at.as_deref(), Some("2026-11-21T12:05:00Z"));
    let pending: Peer = parse(fixture!("peer-disconnect"));
    assert_eq!(pending.state, "revocation_pending");
    let reviews: Page<Review> = parse(fixture!("peer-reviews"));
    assert_eq!(reviews.items.len(), 2);
    let revoked: Message = parse(fixture!("peer-revoked-message"));
    assert_eq!(revoked.state, "revoked");
    let actions: Value = parse(fixture!("peer-actions"));
    for key in [
        "requester_after_decline",
        "requester_after_expiry",
        "requester_after_accept",
    ] {
        serde_json::from_value::<Invitation>(actions[key].clone()).unwrap();
    }
    for key in ["disconnect_requested", "disconnect_confirmed"] {
        serde_json::from_value::<Peer>(actions[key].clone()).unwrap();
    }
}

#[test]
fn client_requests_match_the_service_action_exchanges() {
    use super::super::peers::{Answer, Invitation, Message, Page, Peer, Review};
    let actions: Value = parse(fixture!("peer-actions"));
    let reviews: Page<Review> = parse(fixture!("peer-reviews"));
    let outgoing = reviews.items[0].message.clone();
    let incoming = reviews.items[1].message.clone();
    let exchanges = [
        "invite",
        "accept",
        "decline_invitation",
        "gate",
        "pass_message",
        "decline_message",
        "revise",
        "pass_outgoing",
        "decline_outgoing",
    ];
    let (dir, listener, socket) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), &socket);
    let mut replies = vec![(200, identity)];
    replies.extend(
        exchanges
            .iter()
            .map(|k| (200, actions[k]["response"].clone())),
    );
    replies.push((200, actions["disconnect_requested"].clone()));
    let server = serve(listener, replies);
    let c = profile.connect().unwrap();
    let generation = &incoming.connection.generation;
    let peer_id = actions["invite"]["request"]["peer_id"].as_str().unwrap();
    let invited: Invitation = c.invite_peer(peer_id, "k1").unwrap();
    assert_eq!(invited.state, "pending");
    let accepted: Answer = c.answer_peer(&invited.request_id, true, "k2").unwrap();
    assert!(accepted.connection.is_some());
    assert!(
        c.answer_peer(&invited.request_id, false, "k3")
            .unwrap()
            .connection
            .is_none()
    );
    let gated: Peer = c
        .set_peer_gate(generation, Direction::Incoming, Policy::Auto, "k4")
        .unwrap();
    assert_eq!(gated.gates.incoming, Policy::Auto);
    let decided: Vec<Message> = [
        (&incoming, Decision::Pass),
        (&incoming, Decision::Decline),
        (&outgoing, Decision::Revise("Please include the version.")),
        (&outgoing, Decision::Pass),
        (&outgoing, Decision::Decline),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (m, d))| c.review_peer_message(m, d, &format!("r{i}")).unwrap())
    .collect();
    let states: Vec<_> = decided.iter().map(|m| m.state.as_str()).collect();
    assert_eq!(
        states,
        [
            "passed",
            "declined",
            "revision_requested",
            "passed",
            "declined"
        ]
    );
    assert_eq!(
        c.disconnect_peer(generation, "k5").unwrap().state,
        "revocation_pending"
    );
    let requests = server.join().unwrap();
    let bodies: Vec<Value> = requests[1..]
        .iter()
        .map(|r| serde_json::from_str(r.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    for (body, key) in bodies.iter().zip(exchanges) {
        // The fixture server lowercases what it captures.
        let request = actions[key]["request"].to_string().to_ascii_lowercase();
        assert_eq!(body, &parse::<Value>(&request), "{key}");
    }
    assert_eq!(bodies.last().unwrap(), &json!({}));
    assert!(requests[7].starts_with(&format!(
        "post /api/peer-connections/{generation}/messages/{}/review ",
        outgoing.message_id
    )));
}
