//! Schema-v2 and read-only SQLite compatibility.

use rusqlite::Connection;
use std::path::Path;
use tmux_workspaces_core::{Direction, Node};
use tmux_workspaces_store::{Document, MAX_RECORD_BYTES, Store};

const RECORD: &str = r#"{
  "version": 2, "selected": "000000000001", "focus": false,
  "extension": {"nullable": null, "large": 123456789012345678901234567890},
  "workspaces": [{
    "id": "000000000001", "name": "Development", "icon": "λ",
    "selected": "000000000002", "tabs": [{
      "id": "000000000002", "name": "Editor", "focus": "000000000004",
      "tree": {"id": "000000000003", "direction": "right",
        "first": {"id": "000000000004", "agent": "build worker", "cwd": "/tmp", "source_socket": "/tmp/example.sock"},
        "second": {"id": "000000000005", "agent": null, "empty": true}}
    }]
  }, {"id": "000000000006", "name": "Empty", "tabs": [], "selected": ""}]
}"#;

fn parse(text: &str) -> Result<Document, tmux_workspaces_store::Error> {
    Document::parse(text.to_owned(), Path::new("layouts.db"))
}

fn database(directory: &Path, record: &str) -> Connection {
    let db = Connection::open(directory.join("layouts.db")).unwrap();
    db.execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
        .unwrap();
    db.execute("INSERT INTO terminal_layout VALUES (1, ?1)", [record])
        .unwrap();
    db
}

#[test]
fn schema_v2_keeps_order_attachments_navigation_and_original_json() {
    let document = parse(RECORD).unwrap();
    assert_eq!(document.raw_json(), RECORD);
    let layout = document.model();
    assert_eq!(layout.selected, "000000000001");
    assert!(!layout.show_agents);
    assert!(layout.workspaces[1].tabs.is_empty());
    let tab = &layout.workspaces[0].tabs[0];
    assert_eq!(tab.name, "Editor");
    assert_eq!(tab.focus, "000000000004");
    let Node::Split {
        direction,
        ratio,
        first,
        second,
        ..
    } = &tab.tree
    else {
        panic!()
    };
    assert_eq!(*direction, Direction::Right);
    assert_eq!(*ratio, 0.5);
    let Node::Leaf(first) = first.as_ref() else {
        panic!()
    };
    let attachment = first.attachment.as_ref().unwrap();
    assert_eq!(attachment.session, "build worker");
    assert_eq!(
        attachment.source_socket.as_deref(),
        Some("/tmp/example.sock")
    );
    let Node::Leaf(second) = second.as_ref() else {
        panic!()
    };
    assert!(second.empty);
    assert!(second.cwd.is_none());
}

#[test]
fn rejects_schema_violations_including_unknown_duplicate_fields() {
    for (before, after) in [
        ("\"version\": 2", "\"version\": 2.0"),
        ("\"version\": 2", "\"version\": true"),
        (
            "\"nullable\": null",
            "\"nullable\": null, \"nullable\": true",
        ),
        ("\"nullable\": null", "\"nullable\": NaN"),
        ("\"nullable\": null", "\"nullable\": 1e400"),
        ("\"id\": \"000000000005\"", "\"id\": \"000000000004\""),
        (
            "\"direction\": \"right\"",
            "\"direction\": \"right\", \"ratio\": true",
        ),
        (
            "\"direction\": \"right\"",
            "\"direction\": \"right\", \"ratio\": 1.0",
        ),
        ("\"name\": \"Editor\"", "\"name\": \"\\u00a0\""),
        ("\"agent\": \"build worker\"", "\"agent\": \"build:worker\""),
        ("\"agent\": \"build worker\"", "\"agent\": null"),
        ("\"cwd\": \"/tmp\"", "\"cwd\": null"),
        ("\"cwd\": \"/tmp\"", "\"cwd\": \"relative\""),
        ("\"cwd\": \"/tmp\"", "\"cwd\": \"/tmp\\u0000x\""),
        ("\"empty\": true", "\"empty\": false"),
        ("\"empty\": true", "\"empty\": true, \"ratio\": 0.5"),
        ("\"focus\": \"000000000004\"", "\"focus\": \"000000000003\""),
    ] {
        let text = RECORD.replacen(before, after, 1);
        assert_ne!(text, RECORD);
        assert!(parse(&text).is_err(), "accepted {after}");
    }
}

#[test]
fn bounds_unknown_containers_values_and_record_bytes() {
    // Root at depth zero, extension object at one, array at two.
    let nested = format!("{}null{}", "[".repeat(127), "]".repeat(127));
    assert!(
        parse(&RECORD.replace("\"nullable\": null", &format!("\"nullable\": {nested}"))).is_ok()
    );
    assert!(
        parse(&RECORD.replace("\"nullable\": null", &format!("\"nullable\": [{nested}]"))).is_err()
    );
    let values = vec!["0"; 100_000].join(",");
    assert!(
        parse(&RECORD.replace("\"nullable\": null", &format!("\"nullable\": [{values}]"))).is_err()
    );
    let objects = vec!["{}"; 10_000].join(",");
    assert!(
        parse(&RECORD.replace("\"nullable\": null", &format!("\"nullable\": [{objects}]")))
            .is_err()
    );
    assert!(parse(&" ".repeat(MAX_RECORD_BYTES + 1)).is_err());
}

#[test]
fn readonly_connection_does_not_hold_locks_or_change_saved_data() {
    let directory = tempfile::tempdir().unwrap();
    let db = database(directory.path(), RECORD);
    db.busy_timeout(std::time::Duration::ZERO).unwrap();
    let bytes = std::fs::read(directory.path().join("layouts.db")).unwrap();
    let store = Store::open(directory.path()).unwrap();
    let before = store.data_version().unwrap();
    assert_eq!(store.load().unwrap().raw_json(), RECORD);
    assert_eq!(
        std::fs::read(directory.path().join("layouts.db")).unwrap(),
        bytes
    );
    let changed = RECORD.replace("Development", "Updated");
    db.execute("UPDATE terminal_layout SET value=?1", [&changed])
        .unwrap();
    assert_ne!(store.data_version().unwrap(), before);
    assert_eq!(store.load().unwrap().model().workspaces[0].name, "Updated");
    assert_eq!(
        db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
}

#[test]
fn missing_legacy_malformed_and_oversized_databases_are_not_initialized_or_repaired() {
    let directory = tempfile::tempdir().unwrap();
    assert!(Store::open(directory.path()).is_err());
    assert!(!directory.path().join("layouts.db").exists());
    let db = Connection::open(directory.path().join("layouts.db")).unwrap();
    db.execute_batch("CREATE TABLE layout (id INTEGER PRIMARY KEY, value TEXT)")
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(
        store
            .load()
            .unwrap_err()
            .detail
            .contains("open this library once")
    );
    assert!(!db.table_exists(None, "terminal_layout").unwrap());
    db.execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
        .unwrap();
    assert!(store.load().unwrap_err().detail.contains("row is missing"));
    db.execute(
        "INSERT INTO terminal_layout VALUES (1, ?1)",
        [vec![b'x'; 4]],
    )
    .unwrap();
    assert!(store.load().is_err());
    db.execute(
        "UPDATE terminal_layout SET value=?1",
        [" ".repeat(MAX_RECORD_BYTES + 1)],
    )
    .unwrap();
    assert!(store.load().unwrap_err().detail.contains("16 MiB"));
    assert_eq!(
        db.query_row("SELECT length(value) FROM terminal_layout", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        (MAX_RECORD_BYTES + 1) as i64
    );
}

#[test]
fn split_depth_boundary_matches_the_saved_schema() {
    let mut state: serde_json::Value = serde_json::from_str(RECORD).unwrap();
    for depth in 1..=64 {
        let tree = &mut state["workspaces"][0]["tabs"][0]["tree"];
        *tree = serde_json::json!({
            "id": format!("{:012x}", 100 + depth * 2), "direction": "below", "first": tree.take(),
            "second": {"id": format!("{:012x}", 101 + depth * 2), "agent": null}
        });
        let result = parse(&state.to_string());
        assert_eq!(result.is_ok(), depth < 64, "added depth {depth}");
    }
}
