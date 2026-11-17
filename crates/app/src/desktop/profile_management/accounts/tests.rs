use super::*;
use crate::desktop::profile_management::setup::tests::{served_identity, served_socket};
use serde_json::json;

const SHARED: &str = "Antigravity CLI keeps its login in the machine user's home, so every profile on this machine shares it";

fn stored_as(storage: Storage, message: Option<&str>) -> (String, Option<String>) {
    stored(Ok(Stored {
        storage,
        message: message.map(str::to_owned),
    }))
}

#[test]
fn a_stored_login_reads_as_the_service_says_it() {
    let row = |said: &str, note: Option<&str>| (said.to_owned(), note.map(str::to_owned));
    assert_eq!(
        stored_as(Storage::Present, None),
        row("Signed in (login stored)", None)
    );
    assert_eq!(
        stored_as(Storage::Missing, None),
        row("Not signed in", None)
    );
    assert_eq!(
        stored_as(Storage::Unavailable, Some(SHARED)),
        row(SHARED, None)
    );
    assert_eq!(
        stored_as(Storage::Unavailable, None),
        row("Can't tell whether a login is stored.", None)
    );
    // A note beside a stored login, or none, is shown too.
    assert_eq!(
        stored_as(Storage::Present, Some(SHARED)),
        row("Signed in (login stored)", Some(SHARED))
    );
    assert_eq!(
        stored_as(Storage::Missing, Some(SHARED)),
        row("Not signed in", Some(SHARED))
    );
    // A read that failed says why in the row.
    let missing = "codex is not installed on this machine";
    assert_eq!(
        stored(Err(Failure::Refused(503, missing.into()))),
        row(missing, None)
    );
    assert_eq!(
        stored(Err(Error::Unavailable.into())),
        row("Couldn't reach the profile's service.", None)
    );
}

#[test]
fn a_check_says_its_result_and_when_in_local_time() {
    let at = "2026-11-17T09:00:00Z";
    let local = chrono::DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string();
    let check = |answer, message: Option<&str>, at: &str| {
        checked(Ok(Checked {
            answer,
            message: message.map(str::to_owned),
            checked_at: at.into(),
        }))
    };
    assert_eq!(
        check(Answer::SignedIn, None, at),
        format!("Checked at {local}: signed in.")
    );
    assert_eq!(
        check(Answer::SignedOut, None, at),
        format!("Checked at {local}: not signed in.")
    );
    let late = "Claude Code did not answer within 30 seconds";
    assert_eq!(
        check(Answer::Unknown, Some(late), at),
        format!("Checked at {local}: {late}")
    );
    assert_eq!(
        check(Answer::Unknown, None, at),
        format!("Checked at {local}: couldn't tell whether it's signed in.")
    );
    // The service may write fractions of a second.
    assert_eq!(
        check(Answer::SignedIn, None, "2026-11-17T09:00:00.25Z"),
        format!("Checked at {local}: signed in.")
    );
    // A time that isn't RFC 3339 makes the answer unreadable.
    assert_eq!(
        check(Answer::SignedIn, None, "17 Nov 2026 09:00"),
        "Couldn't read the service's answer."
    );
    assert_eq!(
        checked(Err(Failure::Running)),
        "A check is already running."
    );
}

#[test]
fn a_failure_shows_the_services_words_or_a_sentence_of_ours() {
    assert_eq!(failed(Failure::Running), "A check is already running.");
    for (status, message) in [
        (503, "codex is not installed on this machine"),
        (422, "Antigravity CLI has no status command to ask"),
    ] {
        assert_eq!(failed(Failure::Refused(status, message.into())), message);
    }
    assert_eq!(
        failed(Error::Unavailable.into()),
        "Couldn't reach the profile's service."
    );
    assert_eq!(
        failed(Error::Invalid.into()),
        "Couldn't read the service's answer."
    );
    assert_eq!(
        failed(Error::Refused(403).into()),
        "The service refused it (403)."
    );
    for changed in [
        Error::BindingRequired,
        Error::BindingMismatch,
        Error::Unsupported,
    ] {
        assert_eq!(failed(changed.into()), CHANGED);
    }
}

#[test]
fn sign_in_opens_the_session_the_service_started_and_a_refusal_stays_in_the_row() {
    let started = |created| {
        json!({
            "schema_version": 1, "profile_id": "0a1b2c3d4e5f",
            "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
            "runtime": "codex", "session": "sign-in-codex", "created": created
        })
    };
    let unstarted = "The terminal session could not be started.";
    let (_directory, profile, server) = served_socket(vec![
        (200, served_identity()),
        (200, started(true)),
        (200, started(false)),
        (
            503,
            json!({"detail": {"code": "shell_unavailable", "message": unstarted}}),
        ),
    ]);
    let connection = profile.connect().unwrap();
    // Started now or already running, the pane shows the service's session.
    for _ in 0..2 {
        let Ok(Open::Shell {
            profile_id,
            session,
        }) = opened(&connection, "codex")
        else {
            panic!("the service's session opens");
        };
        assert_eq!(
            (profile_id.as_str(), session.as_str()),
            ("0a1b2c3d4e5f", "sign-in-codex")
        );
    }
    assert!(matches!(opened(&connection, "codex"), Err(why) if why == unstarted));
    let requests = server.join().unwrap();
    assert!(
        requests[1..]
            .iter()
            .all(|request| request.starts_with("post /api/profile/accounts/sign-in ")),
        "{requests:?}"
    );
}
