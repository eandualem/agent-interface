use super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::Command,
    thread,
};

fn id(n: u8) -> Id {
    Id::parse(format!("{n:032x}")).unwrap()
}
fn record(revision: u8) -> serde_json::Value {
    json!({
        "sheet_id":id(1),"title":"Opportunity tracker","author_id":id(2),"author_name":"example-agent","agent_name":"example-agent",
        "revision_id":id(revision),"current_revision_id":id(revision),"parent_revision_id":id(3),
        "created_at":"2026-10-27T00:00:00Z","updated_at":"2026-10-27T00:01:00Z",
        "editor_kind":"operator","editor_id":null,"editor_name":"Operator","calculation_engine":"ironcalc","calculation_version":"0.9.1",
        "tabs":[{"tab_id":id(4),"name":"Opportunities","row_count":2,"column_count":2,"view":{"header_row_id":null,"sort":[],"filters":[]}}],
        "content_sha256":"0".repeat(64)
    })
}
fn request(stream: &mut UnixStream) -> (String, Vec<u8>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() <= 16384);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length = headers
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|s| s.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    (headers, body)
}
fn reply(stream: &mut UnixStream, status: u16, body: serde_json::Value) {
    let body = serde_json::to_vec(&body).unwrap();
    write!(
        stream,
        "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(&body).unwrap();
}

#[test]
fn explicit_types_patch_semantics_and_dates_are_preserved() {
    assert!(Id::parse("../api/messages").is_none());
    assert!(Id::parse("A".repeat(32)).is_none());
    assert!(Input::Date("2024-02-29".into()).valid());
    assert!(!Input::Date("2025-02-29".into()).valid());
    assert!(Input::Date("1900-01-01".into()).valid());
    assert!(!Input::Date("1899-12-31".into()).valid());
    assert!(!Input::Formula("=  ".into()).valid());
    assert!(Input::Formula("=SUM(A1:A2)".into()).valid());
    assert!(!Input::Number(f64::NAN).valid());
    let change = Change {
        row_id: id(5),
        column_id: id(6),
        input: Patch::Set(Input::Text("=literal".into())),
        number_format: Patch::Unchanged,
    };
    let value = serde_json::to_value(&change).unwrap();
    assert_eq!(value["input"], json!({"kind":"text","value":"=literal"}));
    assert!(value.get("number_format").is_none());
    let reset = Change {
        input: Patch::Clear,
        number_format: Patch::Clear,
        ..change
    };
    let value = serde_json::to_value(reset).unwrap();
    assert!(value["input"].is_null());
    assert!(value["number_format"].is_null());
    let blank: Value = serde_json::from_value(json!({"kind":"blank","value":null})).unwrap();
    assert_eq!(blank, Value::Blank(()));
}

#[test]
fn private_http_fixture_checks_lost_reply_retry_and_stale_drafts() {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let worker = thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let (headers, original) = request(&mut first);
        assert!(headers.starts_with(&format!("POST /api/sheets/{}/revisions HTTP/1.1", id(1))));
        let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert!(value["author"].is_null());
        assert_eq!(value["request_id"], json!(id(7)));
        drop(first); // Commit may have happened; deliberately lose the reply.
        let (mut retry, _) = listener.accept().unwrap();
        let (_, retried) = request(&mut retry);
        assert_eq!(original, retried);
        reply(&mut retry, 200, json!({"record":record(8),"created":false}));
        let (mut stale, _) = listener.accept().unwrap();
        request(&mut stale);
        reply(
            &mut stale,
            409,
            json!({"detail":{"code":"stale_revision","sheet_id":id(1),"base_revision_id":id(3),"current_revision_id":id(8)}}),
        );
        for limit in [50, 25] {
            let (mut stream, _) = listener.accept().unwrap();
            let (headers, body) = request(&mut stream);
            assert!(headers.starts_with(&format!(
                "GET /api/sheets?limit={limit}&cursor=saved%2Bcursor&author_id={} HTTP/1.1",
                id(2)
            )));
            assert!(body.is_empty());
            if limit == 50 {
                reply(
                    &mut stream,
                    413,
                    json!({"detail":{"code":"response_too_large"}}),
                );
            } else {
                reply(
                    &mut stream,
                    200,
                    json!({"items":[record(8)],"next_cursor":"next-page"}),
                );
            }
        }
        // A single record that still cannot fit must stop, retaining the cursor.
        for limit in [50, 25, 13, 7, 4, 2, 1] {
            let (mut stream, _) = listener.accept().unwrap();
            let (headers, _) = request(&mut stream);
            assert!(headers.starts_with(&format!(
                "GET /api/sheets/{}/revisions?limit={limit}&cursor=history%2Bcursor HTTP/1.1",
                id(1)
            )));
            reply(
                &mut stream,
                413,
                json!({"detail":{"code":"response_too_large"}}),
            );
        }
        // Other refusals are not permission to issue automatic retries.
        let (mut stream, _) = listener.accept().unwrap();
        let (headers, _) = request(&mut stream);
        assert!(headers.starts_with(&format!(
            "GET /api/sheets/{}/revisions?limit=50 HTTP/1.1",
            id(1)
        )));
        reply(
            &mut stream,
            413,
            json!({"detail":{"code":"metadata_too_large"}}),
        );
        // The shelf follows all pages; cyclic cursors must stop promptly.
        for (cursor, next, revision) in [
            ("", Some("page2"), 8),
            ("&cursor=page2", None, 9),
            ("", Some("loop"), 8),
            ("&cursor=loop", Some("loop"), 9),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let (headers, _) = request(&mut stream);
            assert!(headers.starts_with(&format!("GET /api/sheets?limit=50{cursor} HTTP/1.1")));
            reply(
                &mut stream,
                200,
                json!({"items":[record(revision)], "next_cursor":next}),
            );
        }
    });
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "sheets::tests::transport_fixture_child",
            "--nocapture",
        ])
        .env("SHEETS_FIXTURE_DIRECTORY", directory.path())
        .env("BACKBONE_SOCKET_PATH", url.strip_prefix("unix:").unwrap())
        .env("BACKBONE_API_KEY", "fixture-token")
        .status()
        .unwrap();
    assert!(status.success());
    worker.join().unwrap();
}

#[test]
fn transport_fixture_child() {
    let Some(directory) = std::env::var_os("SHEETS_FIXTURE_DIRECTORY") else {
        return;
    };
    let client = Client::connect(Path::new(&directory)).unwrap();
    let edit = Edit::operator(
        id(7),
        id(3),
        vec![Operation::SetCells {
            tab_id: id(4),
            cells: vec![Change {
                row_id: id(5),
                column_id: id(6),
                input: Patch::Set(Input::Boolean(true)),
                number_format: Patch::Unchanged,
            }],
        }],
    );
    let prepared = client.prepare_edit(&id(1), &edit).unwrap();
    assert!(matches!(client.submit(&prepared), Err(Error::Unknown)));
    assert!(matches!(client.submit(&prepared),Ok(Accepted::Revision(r)) if r.revision_id == id(8)));
    assert_eq!(
        client.submit(&prepared).unwrap_err(),
        Error::Stale { current: id(8) }
    );
    assert_eq!(edit.base_revision_id, id(3));
    assert_eq!(prepared.body, serde_json::to_vec(&edit).unwrap());
    let page = client
        .list(Some(&id(2).to_string()), Some("saved+cursor"))
        .unwrap();
    assert_eq!(page.items[0].revision_id, id(8));
    assert_eq!(page.next_cursor.as_deref(), Some("next-page"));
    assert!(matches!(
        client.history(&id(1), Some("history+cursor")),
        Err(Error::ResponseTooLarge)
    ));
    assert!(matches!(
        client.history(&id(1), None),
        Err(Error::Refused(413))
    ));
    let all = client.list_all().unwrap();
    assert_eq!(
        all.iter()
            .map(|record| record.revision_id.clone())
            .collect::<Vec<_>>(),
        vec![id(8), id(9)]
    );
    assert!(matches!(client.list_all(), Err(Error::Unreadable)));
}

#[test]
fn feedback_does_not_conflate_uncertain_delivery_with_queued_delivery() {
    let mut delivery: Delivery = serde_json::from_value(json!({
        "ok":false,"queued":true,"outcome":"uncertain"
    }))
    .unwrap();
    assert!(delivery.message().contains("uncertain"));
    assert!(!delivery.message().contains("queued"));
    let mut messages = std::collections::HashSet::new();
    for outcome in ["uncertain", "queued", "expired", "unknown"] {
        delivery.outcome = outcome.into();
        delivery.queued = matches!(outcome, "uncertain" | "queued");
        assert!(messages.insert(delivery.message()));
    }
    delivery.outcome = "delivered".into();
    delivery.ok = true;
    assert!(
        delivery
            .message()
            .contains("An answer has not been received")
    );
}

#[test]
fn copied_uncertain_feedback_receipt_keeps_context_and_operation_identity() {
    let receipt: FeedbackReceipt = serde_json::from_value(json!({
        "feedback_id":id(1),"sheet_id":id(2),"observed_revision_id":id(3),
        "operation_id":"original-operation", "delivery":{"ok":false,"queued":true,"outcome":"uncertain"}
    })).unwrap();
    let copied = serde_json::to_value(receipt).unwrap();
    assert_eq!(copied["operation_id"], "original-operation");
    assert_eq!(copied["feedback_id"], json!(id(1)));
    assert_eq!(copied["sheet_id"], json!(id(2)));
    assert_eq!(copied["observed_revision_id"], json!(id(3)));
    assert_eq!(copied["delivery"]["outcome"], "uncertain");
}

#[test]
fn sparse_sorted_cells_keep_canonical_identity_and_reject_duplicates() {
    let wire = json!({"sheet_id":id(1),"revision_id":id(3),"tab_id":id(4),"order":"view","total_rows":2,"total_columns":1,
        "rows":[{"row_id":id(6),"index":2},{"row_id":id(5),"index":1}],
        "columns":[{"column_id":id(7),"index":1}],"next_row_offset":null,
        "cells":[{"row_id":id(6),"column_id":id(7),"input":{"kind":"formula","value":"=1/0"},"value":{"kind":"error","value":"#DIV/0!"},"display":"#DIV/0!"}]});
    let mut grid: Grid = serde_json::from_value(wire).unwrap();
    assert!(grid.valid());
    assert_eq!(grid.rows[0].index, 2);
    assert!(grid.cell(&id(5), &id(7)).is_none());
    grid.cells.push(grid.cells[0].clone());
    assert!(!grid.valid());
}

#[test]
fn diagnostics_limits_count_ascii_escaping_without_discarding_messages() {
    assert!(valid_warnings(&[
        "Unsupported decorative formatting was omitted.".into()
    ]));
    assert!(!valid_warnings(&vec!["warning".into(); 9]));
    assert!(!valid_warnings(&["x".repeat(513)]));
    assert!(!valid_warnings(&["warning\nhidden".into()]));
    assert!(!valid_warnings(&vec!["語".repeat(170); 8]));
}

#[test]
fn range_footprints_and_feedback_bounds_fail_before_a_request() {
    let feedback = Feedback::operator(
        id(7),
        id(3),
        FeedbackKind::Question,
        "What changed?".into(),
        Some(Anchor {
            tab_id: id(4),
            row_ids: (0..11).map(|n| id(20 + n)).collect(),
            column_ids: (0..10).map(|n| id(50 + n)).collect(),
        }),
    );
    assert!(!feedback.valid());
    let edit = Edit::operator(
        id(7),
        id(3),
        vec![Operation::SetCells {
            tab_id: id(4),
            cells: (0..1001)
                .map(|_| Change {
                    row_id: id(5),
                    column_id: id(6),
                    input: Patch::Set(Input::Boolean(true)),
                    number_format: Patch::Unchanged,
                })
                .collect(),
        }],
    );
    assert!(!edit.valid());
}

#[test]
fn provider_endpoint_fixtures_decode_without_losing_revision_or_delivery_identity() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/sheets-api.json")).unwrap();
    let initial: Mutation = serde_json::from_value(fixtures["create"].clone()).unwrap();
    assert!(initial.record.valid());
    for name in ["list", "history"] {
        let page: Page = serde_json::from_value(fixtures[name].clone()).unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items.iter().all(Record::valid));
        assert_eq!(page.items[0].revision_id, initial.record.revision_id);
    }
    let metadata: Record = serde_json::from_value(fixtures["metadata"].clone()).unwrap();
    let range: Grid = serde_json::from_value(fixtures["range"].clone()).unwrap();
    assert!(metadata.valid());
    assert!(range.valid());
    assert_eq!(range.revision_id, metadata.revision_id);
    assert_eq!(range.rows.len(), 5);
    assert_eq!(range.cells.len(), 24); // The completely empty fifth row remains addressable.
    let edited: Mutation = serde_json::from_value(fixtures["edit"].clone()).unwrap();
    assert_eq!(
        edited.record.parent_revision_id,
        Some(initial.record.revision_id.clone())
    );
    assert_eq!(edited.record.editor_kind, "operator");
    assert!(edited.record.editor_id.is_none());
    let stale: FailureEnvelope = serde_json::from_value(fixtures["stale"].clone()).unwrap();
    assert_eq!(stale.detail.code, "stale_revision");
    assert_eq!(
        stale.detail.current_revision_id,
        Some(edited.record.revision_id)
    );
    let imported: Imported = serde_json::from_value(fixtures["import"].clone()).unwrap();
    assert!(imported.record.valid());
    assert!(valid_warnings(&imported.warnings));
    assert_eq!(imported.warnings.len(), 1);
    let feedback: FeedbackReceipt = serde_json::from_value(fixtures["feedback"].clone()).unwrap();
    assert_eq!(feedback.observed_revision_id, initial.record.revision_id);
    assert!(feedback.delivery.queued);
    assert!(!feedback.delivery.ok);
    assert_eq!(feedback.delivery.outcome, "queued");
}
