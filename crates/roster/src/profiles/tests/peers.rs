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
        Err(Error::Invalid)
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
