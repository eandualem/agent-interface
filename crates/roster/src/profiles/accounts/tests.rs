use super::*;
use crate::profiles::{runtime_catalog::Authentication, secrets::tests::served};
use serde_json::{Value as Json, json};

/// The service's own fixtures, copied byte for byte.
fn fixture(name: &str) -> Json {
    serde_json::from_str(match name {
        "account-sign-in" => include_str!("fixtures/account-sign-in.json"),
        "account-sign-in-started" => include_str!("fixtures/account-sign-in-started.json"),
        "account-status" => include_str!("fixtures/account-status.json"),
        "account-status-missing" => include_str!("fixtures/account-status-missing.json"),
        "account-status-unavailable" => include_str!("fixtures/account-status-unavailable.json"),
        "account-check" => include_str!("fixtures/account-check.json"),
        "account-check-signed-in" => include_str!("fixtures/account-check-signed-in.json"),
        "account-check-signed-out" => include_str!("fixtures/account-check-signed-out.json"),
        "account-check-unknown" => include_str!("fixtures/account-check-unknown.json"),
        _ => panic!("no fixture {name}"),
    })
    .unwrap()
}

/// A request's head, lower-cased and ending in a line break, and its body.
fn split(request: &str) -> (String, &str) {
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    (format!("{head}\r\n").to_ascii_lowercase(), body)
}

fn refusal(code: &str, message: &str) -> Json {
    json!({"detail": {"code": code, "message": message}})
}

#[test]
fn stored_logins_decode_from_the_services_fixtures() {
    let (connection, server) = served(vec![
        (200, fixture("account-status")),
        (200, fixture("account-status-missing")),
        (200, fixture("account-status-unavailable")),
    ]);
    let stored = |storage, message: Option<&str>| {
        Ok::<_, Failure>(Stored {
            storage,
            message: message.map(str::to_owned),
        })
    };
    assert_eq!(
        connection.stored_login("claude"),
        stored(Storage::Present, None)
    );
    assert_eq!(
        connection.stored_login("codex"),
        stored(Storage::Missing, None)
    );
    let shared = fixture("account-status-unavailable")["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        connection.stored_login("antigravity"),
        stored(Storage::Unavailable, Some(shared.as_str()))
    );
    let requests = server.join().unwrap();
    for (request, runtime) in requests[1..].iter().zip(["claude", "codex", "antigravity"]) {
        let line = format!("GET /api/profile/accounts/status?runtime={runtime} HTTP/1.1\r\n");
        assert!(request.starts_with(&line), "{request}");
    }
}

#[test]
fn a_sign_in_sends_only_the_cli_and_returns_the_services_session() {
    let mut running = fixture("account-sign-in-started");
    running["created"] = json!(false);
    let (connection, server) = served(vec![
        (200, fixture("account-sign-in-started")),
        (200, running),
    ]);
    let started = |created| {
        Ok::<_, Failure>(Started {
            session: "sign-in-codex".into(),
            created,
        })
    };
    assert_eq!(connection.start_sign_in("codex"), started(true));
    // One already running is an answer too, with the same session.
    assert_eq!(connection.start_sign_in("codex"), started(false));
    let requests = server.join().unwrap();
    let (head, body) = split(&requests[1]);
    assert!(head.starts_with("post /api/profile/accounts/sign-in http/1.1\r\n"));
    let sent: Json = serde_json::from_str(body).unwrap();
    assert_eq!(sent, fixture("account-sign-in"));
}

#[test]
fn a_check_carries_no_request_id_and_reads_each_result() {
    let mut later = fixture("account-check-signed-in");
    later["result"] = json!("expired");
    let (connection, server) = served(vec![
        (200, fixture("account-check-signed-in")),
        (200, fixture("account-check-signed-out")),
        (200, fixture("account-check-unknown")),
        (200, later),
    ]);
    let checked = |answer, message: Option<&str>| {
        Ok::<_, Failure>(Checked {
            answer,
            message: message.map(str::to_owned),
            checked_at: "2026-11-17T09:00:00Z".into(),
        })
    };
    assert_eq!(
        connection.check_sign_in("claude"),
        checked(Answer::SignedIn, None)
    );
    assert_eq!(
        connection.check_sign_in("codex"),
        checked(Answer::SignedOut, None)
    );
    assert_eq!(
        connection.check_sign_in("claude"),
        checked(
            Answer::Unknown,
            Some("Claude Code did not answer within 30 seconds")
        )
    );
    // A result this app doesn't know reads as unknown.
    assert_eq!(
        connection.check_sign_in("claude"),
        checked(Answer::Unknown, None)
    );
    let requests = server.join().unwrap();
    let (head, body) = split(&requests[1]);
    assert!(head.starts_with("post /api/profile/accounts/check http/1.1\r\n"));
    assert!(!head.contains("x-backbone-request-id"), "{head}");
    let sent: Json = serde_json::from_str(body).unwrap();
    assert_eq!(sent, fixture("account-check"));
    // The service ends a check after 30 seconds; the app waits a little longer.
    assert_eq!(CHECK_WAIT, Duration::from_secs(35));
}

#[test]
fn a_running_check_is_told_apart_and_a_refusal_keeps_the_services_words() {
    let missing = "codex is not installed on this machine";
    let antigravity = "Antigravity CLI has no status command to ask";
    let (connection, server) = served(vec![
        (409, refusal("conflict", "claude is being checked")),
        (503, refusal("unavailable", missing)),
        (422, refusal("invalid_request", antigravity)),
        (403, json!({})),
    ]);
    assert_eq!(connection.check_sign_in("claude"), Err(Failure::Running));
    assert_eq!(
        connection.check_sign_in("codex"),
        Err(Failure::Refused(503, missing.into()))
    );
    assert_eq!(
        connection.check_sign_in("antigravity"),
        Err(Failure::Refused(422, antigravity.into()))
    );
    // A refusal without a reason keeps only its status.
    assert_eq!(
        connection.stored_login("codex"),
        Err(Failure::Connection(Error::Refused(403)))
    );
    server.join().unwrap();
}

#[test]
fn an_answer_about_another_profile_cli_or_session_is_refused() {
    let mut foreign = fixture("account-status");
    foreign["profile_id"] = json!("9f8e7d6c5b4a");
    let mut other = fixture("account-check-signed-in");
    other["runtime"] = json!("codex");
    let mut session = fixture("account-sign-in-started");
    session["session"] = json!("sign-in-claude");
    let mut storage = fixture("account-status");
    storage["storage"] = json!("expired");
    let mut newer = fixture("account-status");
    newer["schema_version"] = json!(2);
    let (connection, server) = served(vec![
        (200, foreign),
        (200, other),
        (200, session),
        (200, storage),
        (200, newer),
        (200, json!("not an answer")),
    ]);
    let unreadable = Some(Failure::Connection(Error::Invalid));
    assert_eq!(
        connection.stored_login("claude").err(),
        Some(Failure::Connection(Error::BindingMismatch))
    );
    assert_eq!(connection.check_sign_in("claude").err(), unreadable);
    assert_eq!(connection.start_sign_in("codex").err(), unreadable);
    // A stored login is one of three answers, in schema 1.
    assert_eq!(connection.stored_login("claude").err(), unreadable);
    assert_eq!(connection.stored_login("claude").err(), unreadable);
    assert_eq!(connection.check_sign_in("claude").err(), unreadable);
    server.join().unwrap();
}

#[test]
fn a_sign_in_session_uses_the_services_route_and_keeps_a_refusals_status() {
    let unstarted = refusal(
        "shell_unavailable",
        "The terminal session could not be started.",
    );
    let (connection, server) = served(vec![
        (200, fixture("account-sign-in-started")),
        (503, unstarted),
    ]);
    assert_eq!(
        connection.sign_in_session("codex").as_deref(),
        Ok("sign-in-codex")
    );
    assert_eq!(
        connection.sign_in_session("codex"),
        Err(Error::Refused(503))
    );
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("POST /api/profile/accounts/sign-in HTTP/1.1\r\n"));
}

#[test]
fn the_status_read_uses_the_services_route_and_keeps_a_refusals_status() {
    let unknown = refusal(
        "invalid_request",
        "\"gemini\" is not claude, codex or antigravity",
    );
    let (connection, server) = served(vec![
        (200, fixture("account-status")),
        (200, fixture("account-status-missing")),
        (200, fixture("account-status-unavailable")),
        (422, unknown),
    ]);
    assert_eq!(
        connection.account_status("claude"),
        Ok(Authentication::Authenticated)
    );
    assert_eq!(
        connection.account_status("codex"),
        Ok(Authentication::NotAuthenticated)
    );
    assert_eq!(
        connection.account_status("antigravity"),
        Ok(Authentication::Unavailable)
    );
    assert_eq!(
        connection.account_status("gemini"),
        Err(Error::Refused(422))
    );
    let requests = server.join().unwrap();
    assert!(
        requests[1].starts_with("GET /api/profile/accounts/status?runtime=claude HTTP/1.1\r\n")
    );
}
