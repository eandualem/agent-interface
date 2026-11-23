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

/// `roster` as if it had been read at `at`.
fn read_at(roster: &Snapshot, at: Instant) -> Snapshot {
    Snapshot {
        observed_at: Some(at),
        ..roster.clone()
    }
}

const REVIEW: &str = "0193a1b2c3d47e5f8a9b0c1d2e3f4a60";
const BUILD: &str = "0193a1b2c3d47e5f8a9b0c1d2e3f4a61";
const THIRD: &str = "0193a1b2c3d47e5f8a9b0c1d2e3f4a62";

/// One read as the worker makes it: the list, then the runs that left it.
fn follow(book: &mut Book, connection: &Connection, roster: &Snapshot) {
    let gone = book.list(connection, roster);
    book.settle(connection, gone, &|| true);
}

fn ids(runs: &[Run]) -> Vec<String> {
    runs.iter().map(|run| run.run_id.clone()).collect()
}

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
        (
            404,
            json!({"detail": {"code": "not_found", "message": "No run."}}),
        ),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let idle = roster(br#"{"items":[{"name":"worker","state":"idle","background":{"running":0,"queued":0,"processes":0}}]}"#);
    let mut book = Book::default();
    follow(&mut book, &connection, &busy);
    assert_eq!(book.runs().active, decoded(fixture("runs")));
    assert!(book.runs().finished.is_empty());
    follow(&mut book, &connection, &busy);
    assert_eq!(ids(&book.runs().active), [BUILD]);
    assert_eq!(book.runs().finished.len(), 1);
    assert_eq!(book.runs().finished[0].exit_code, Some(101));
    // No agent reports runs, but the book still held one, so it asks once more.
    follow(&mut book, &connection, &idle);
    assert!(book.runs().active.is_empty());
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    // Nothing is active and nobody reports runs: nothing is asked.
    follow(&mut book, &connection, &idle);
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
fn a_service_without_runs_is_asked_once() {
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        (200, json!([[]])),
        (503, json!({})),
        (404, json!({})),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    // An older service lists agents without counts.
    let older = roster(br#"{"items":[{"name":"worker","state":"busy"}]}"#);
    follow(&mut book, &connection, &older);
    follow(&mut book, &connection, &busy);
    assert_eq!(book.runs().active.len(), 2);
    // A list that isn't an object, or a failed read, keeps the book and
    // starts no lookup.
    follow(&mut book, &connection, &busy);
    assert_eq!(book.runs().active.len(), 2);
    follow(&mut book, &connection, &busy);
    assert_eq!(book.runs().active.len(), 2);
    // A service without the route has nothing running and isn't asked again.
    follow(&mut book, &connection, &busy);
    follow(&mut book, &connection, &busy);
    assert_eq!(book.runs(), &Runs::default());
    assert_eq!(server.join().unwrap().len(), 5);
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
    // An array is not a run or a review, though either would decode from one.
    let positional = json!([BUILD, null, "builder", "build", "cargo build", "running"]);
    let mut listed_review = fixture("run");
    listed_review["run_id"] = json!(THIRD);
    listed_review["review"] = json!(["codex", "gpt-example", "high"]);
    // A body nested deeper than the roster allows is refused before decoding.
    let mut deep = fixture("run");
    deep["progress"] = (0..129).fold(json!(0), |inner, _| json!([inner]));
    let (connection, server) = served(vec![
        (200, other),
        (200, deep),
        (200, positional.clone()),
        (
            200,
            json!({"items": [{"run_id": REVIEW}, mistyped, {"kind": "test"}, positional, listed_review]}),
        ),
        (200, json!({"items": {}})),
    ]);
    assert_eq!(connection.run(REVIEW), Err(Error::Invalid));
    assert_eq!(connection.run(REVIEW), Err(Error::Invalid));
    assert_eq!(connection.run(BUILD), Err(Error::Invalid));
    let listed = connection.runs().unwrap();
    assert_eq!(ids(&listed), [REVIEW, THIRD]);
    assert_eq!(listed[1].review, None);
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

#[test]
fn the_new_list_shows_first_and_a_stop_or_the_deadline_starts_no_lookup() {
    let mut ended = fixture("run");
    ended["state"] = json!("finished");
    ended["finished_at"] = json!("2026-11-10T10:21:47Z");
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        (200, json!({"items": []})),
        (200, ended),
        (
            404,
            json!({"detail": {"code": "not_found", "message": "No run."}}),
        ),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    follow(&mut book, &connection, &busy);
    let gone = book.list(&connection, &busy);
    // Both runs left the list, which shows before either is read.
    assert_eq!(book.runs(), &Runs::default());
    assert_eq!(ids(&gone.runs), [REVIEW, BUILD]);
    // A stopped worker starts no lookup, and neither does one past its deadline.
    let stopped = Gone {
        runs: gone.runs.clone(),
        deadline: gone.deadline,
    };
    book.settle(&connection, stopped, &|| false);
    let late = Gone {
        runs: gone.runs.clone(),
        deadline: Some(Instant::now()),
    };
    book.settle(&connection, late, &|| true);
    assert!(book.runs().finished.is_empty());
    // With time left each is read once; the one that can't be read is forgotten.
    book.settle(&connection, gone, &|| true);
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests[3].starts_with(&format!("GET /api/runs/{REVIEW} HTTP/1.1\r\n")));
    assert!(requests[4].starts_with(&format!("GET /api/runs/{BUILD} HTTP/1.1\r\n")));
}

#[test]
fn runs_that_end_in_one_read_are_kept_newest_first() {
    let run = |id: &str, state: &str, finished: Json| {
        let mut run = fixture("run");
        run["run_id"] = json!(id);
        run["state"] = json!(state);
        run["finished_at"] = finished;
        run
    };
    let (connection, server) = served(vec![
        (
            200,
            json!({"items": [
                run(REVIEW, "running", Json::Null),
                run(BUILD, "running", Json::Null),
                run(THIRD, "running", Json::Null),
            ]}),
        ),
        (200, json!({"items": []})),
        // Read in list order, they ended in another; a time that doesn't
        // parse counts as oldest.
        (200, run(REVIEW, "finished", json!("2026-11-10T10:20:00Z"))),
        (200, run(BUILD, "failed", json!("2026-11-10T10:25:00Z"))),
        (200, run(THIRD, "cancelled", json!("soon"))),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    follow(&mut book, &connection, &busy);
    follow(&mut book, &connection, &busy);
    assert_eq!(ids(&book.runs().finished), [BUILD, REVIEW, THIRD]);
    server.join().unwrap();
}

#[test]
fn a_later_run_without_an_end_time_stays_behind_dated_ones() {
    let mut dated = fixture("run");
    dated["state"] = json!("finished");
    dated["finished_at"] = json!("2026-11-10T10:21:47Z");
    let mut undated = fixture("runs")["items"][1].clone();
    undated["state"] = json!("cancelled");
    let build = fixture("runs")["items"][1].clone();
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        (200, json!({"items": [build]})),
        (200, dated),
        (200, json!({"items": []})),
        (200, undated),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    for _ in 0..3 {
        follow(&mut book, &connection, &busy);
    }
    assert_eq!(ids(&book.runs().finished), [REVIEW, BUILD]);
    server.join().unwrap();
}

#[test]
fn a_list_unread_for_30_seconds_is_stale_until_a_good_read_settles_it() {
    let mut ended = fixture("run");
    ended["state"] = json!("finished");
    ended["finished_at"] = json!("2026-11-10T10:21:47Z");
    let build = fixture("runs")["items"][1].clone();
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        (503, json!({})),
        (503, json!({})),
        (503, json!({})),
        // The review ended while the list could not be read.
        (200, json!({"items": [build]})),
        (200, ended),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let builder = "0193a1b2c3d47e5f8a9b0c1d2e3f4a5b";
    let mut book = Book::default();
    let start = Instant::now();
    let read = |book: &mut Book, after: u64| {
        let now = start + Duration::from_secs(after);
        let gone = book.list_at(&connection, &read_at(&busy, now), now);
        book.settle(&connection, gone, &|| true);
    };
    read(&mut book, 0);
    // Counted from the last good read: shown until 30 seconds have passed.
    read(&mut book, 10);
    read(&mut book, 29);
    assert!(!book.runs().stale);
    assert_eq!(book.runs().of(builder).active.len(), 1);
    read(&mut book, 30);
    assert!(book.runs().stale);
    assert!(book.runs().of(builder).active.is_empty());
    // The list is kept inside, so the next good read still finds the run
    // that ended and shows the list again.
    assert_eq!(book.runs().active.len(), 2);
    read(&mut book, 31);
    assert!(!book.runs().stale);
    assert_eq!(ids(&book.runs().active), [BUILD]);
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    assert_eq!(server.join().unwrap().len(), 7);
}

#[test]
fn a_roster_outage_counts_toward_a_stale_list_until_a_good_list_read() {
    let review = fixture("runs")["items"][0].clone();
    let mut ended = fixture("run");
    ended["state"] = json!("finished");
    ended["finished_at"] = json!("2026-11-10T10:21:47Z");
    let (connection, server) = served(vec![
        (200, json!({"items": [review]})),
        (503, json!({})),
        (200, json!({"items": []})),
        (200, ended),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    let start = Instant::now();
    let at = |after: u64| start + Duration::from_secs(after);
    let gone = book.list_at(&connection, &read_at(&busy, at(0)), at(0));
    book.settle(&connection, gone, &|| true);
    // Roster reads that take their whole timeout and fail: the list goes
    // stale 30 seconds after the roster read its last good read followed,
    // as the roster does.
    book.expire_at(at(17));
    assert!(!book.runs().stale);
    book.expire_at(at(34));
    assert!(book.runs().stale);
    // The roster is back but the list fails: it stays stale.
    let gone = book.list_at(&connection, &read_at(&busy, at(36)), at(36));
    book.settle(&connection, gone, &|| true);
    assert!(book.runs().stale);
    // A good list clears it and still settles the review that ended.
    let gone = book.list_at(&connection, &read_at(&busy, at(38)), at(38));
    book.settle(&connection, gone, &|| true);
    assert!(!book.runs().stale);
    assert!(book.runs().active.is_empty());
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    assert_eq!(server.join().unwrap().len(), 5);
}

#[test]
fn an_empty_book_is_never_stale() {
    let (connection, server) = served(vec![(200, json!({"items": []}))]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let start = Instant::now();
    let mut never_read = Book::default();
    never_read.expire_at(start + Duration::from_secs(60));
    assert!(!never_read.runs().stale);
    // A good read of an empty list, then a minute of missed reads.
    let mut book = Book::default();
    let gone = book.list_at(&connection, &read_at(&busy, start), start);
    book.settle(&connection, gone, &|| true);
    for after in [30, 60] {
        book.expire_at(start + Duration::from_secs(after));
    }
    assert_eq!(book.runs(), &Runs::default());
    server.join().unwrap();
}

#[test]
fn the_30_seconds_run_from_the_roster_read_and_hold_from_the_start_of_a_pass() {
    let review = fixture("runs")["items"][0].clone();
    let mut ended = fixture("run");
    ended["state"] = json!("finished");
    ended["finished_at"] = json!("2026-11-10T10:21:47Z");
    let (connection, server) = served(vec![
        (200, json!({"items": [review]})),
        (200, json!({"items": []})),
        (200, ended),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    let start = Instant::now();
    let at = |after: u64| start + Duration::from_secs(after);
    // The roster read at 0 s took 10 s: the list read that followed it
    // counts from the roster read, as the roster itself does.
    let gone = book.list_at(&connection, &read_at(&busy, at(0)), at(10));
    book.settle(&connection, gone, &|| true);
    book.expire_at(at(29));
    assert!(!book.runs().stale);
    // A pass that starts at 36 s is stale before any list read, so the
    // roster it publishes first already hides the old runs.
    book.expire_at(at(36));
    assert!(book.runs().stale);
    // Its good list read clears that and settles the review that ended.
    let gone = book.list_at(&connection, &read_at(&busy, at(36)), at(36));
    book.settle(&connection, gone, &|| true);
    assert!(!book.runs().stale);
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    assert_eq!(server.join().unwrap().len(), 4);
}

#[test]
fn a_run_shows_only_its_own_view_and_views_are_named_by_run() {
    let mut run: Run = serde_json::from_value(fixture("run")).unwrap();
    assert_eq!(run.view(), None);
    run.session = Some(format!("run-{REVIEW}"));
    assert_eq!(run.view(), Some(format!("run-{REVIEW}").as_str()));
    // Another run's view, or any other session, is not this run's.
    for other in [format!("run-{BUILD}"), "worker".into(), REVIEW.into()] {
        run.session = Some(other);
        assert_eq!(run.view(), None);
    }
    // An ID not named as views are is never offered as one.
    run.run_id = REVIEW.to_uppercase();
    run.session = Some(format!("run-{}", run.run_id));
    assert_eq!(run.view(), None);
    assert!(is_view(&format!("run-{REVIEW}")));
    for loose in [
        "run-".to_owned(),
        "run-tests".into(),
        format!("run-{}", REVIEW.to_uppercase()),
        format!("run-{REVIEW}0"),
        format!("shell-{REVIEW}"),
    ] {
        assert!(!is_view(&loose), "{loose}");
    }
}

#[test]
fn an_ended_runs_view_is_offered_until_the_service_ends_it() {
    let mut ended = fixture("run");
    ended["state"] = json!("finished");
    ended["session"] = json!(format!("run-{REVIEW}"));
    let mut gone = ended.clone();
    gone["session"] = json!(null);
    let build = json!({"items": [fixture("runs")["items"][1].clone()]});
    let (connection, server) = served(vec![
        (200, fixture("runs")),
        (200, build.clone()),
        (200, ended),
        (200, build),
        // 30 seconds on, the view is read again.
        (200, gone),
    ]);
    let busy = roster(include_bytes!("fixtures/agents.json"));
    let mut book = Book::default();
    let start = Instant::now();
    for seconds in [0, 1, 2] {
        let at = start + Duration::from_secs(seconds);
        let left = book.list_at(&connection, &busy, at);
        book.settle(&connection, left, &|| true);
    }
    assert!(book.runs().finished[0].view().is_some());
    // A pass that is closing reads nothing.
    book.review_views(&connection, start + Duration::from_secs(31), &|| false);
    assert!(book.runs().finished[0].view().is_some());
    book.viewed_at = None;
    book.review_views(&connection, start + Duration::from_secs(31), &|| true);
    assert_eq!(ids(&book.runs().finished), [REVIEW]);
    assert_eq!(book.runs().finished[0].view(), None);
    let requests = server.join().unwrap();
    assert!(requests[5].starts_with(&format!("GET /api/runs/{REVIEW} HTTP/1.1\r\n")));
    assert_eq!(requests.len(), 6);
}

#[test]
fn the_oldest_ended_views_are_read_again_first() {
    // Nine ended runs with views, newest first; a round reads eight.
    let runs: Vec<Run> = (0..9u8)
        .map(|n| {
            let mut run: Run = serde_json::from_value(fixture("run")).unwrap();
            run.run_id = format!("{:0>32}", n);
            run.session = Some(format!("run-{}", run.run_id));
            run
        })
        .collect();
    let replies = runs[1..]
        .iter()
        .rev()
        .map(|run| {
            let mut gone = serde_json::to_value(fixture("run")).unwrap();
            gone["run_id"] = json!(run.run_id);
            gone["session"] = json!(null);
            (200, gone)
        })
        .collect();
    let (connection, server) = served(replies);
    let mut book = Book::default();
    book.runs.finished = runs.clone();
    book.review_views(&connection, Instant::now(), &|| true);
    assert_eq!(book.runs.finished[0].view(), runs[0].view());
    assert!(
        book.runs.finished[1..]
            .iter()
            .all(|run| run.view().is_none())
    );
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with(&format!("GET /api/runs/{} ", runs[8].run_id)));
}
