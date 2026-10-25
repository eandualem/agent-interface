//! Schema-v2 and read-only SQLite compatibility.

use rusqlite::Connection;
use std::path::Path;
use tmux_workspaces_core::{AgentSection, Direction, Node, View, ViewKind};
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
fn transactional_edits_merge_and_conflicts_reset_before_an_explicit_retry() {
    let directory = tempfile::tempdir().unwrap();
    let db = database(directory.path(), RECORD);
    db.execute_batch("CREATE TABLE extension_row(parent INTEGER REFERENCES terminal_layout(id) ON DELETE CASCADE, value TEXT); INSERT INTO extension_row VALUES(1, 'retained')").unwrap();
    let mut first = Store::open_existing_writable(directory.path()).unwrap();
    let mut second = Store::open_existing_writable(directory.path()).unwrap();
    let mut first_base = first.load().unwrap();
    let mut second_base = second.load().unwrap();
    let mut a = first_base.model().clone();
    let mut b = second_base.model().clone();
    a.workspaces[0].name = "Workspace edit".into();
    b.workspaces[0].tabs[0].name = "Tab edit".into();
    b.selected = b.workspaces[1].id.clone();
    assert!(!first.save(&mut first_base, &a).unwrap().conflict);
    let saved = second.save(&mut second_base, &b).unwrap();
    assert!(!saved.conflict);
    assert_eq!(saved.document.model().workspaces[0].name, "Workspace edit");
    assert_eq!(
        saved.document.model().workspaces[0].tabs[0].name,
        "Tab edit"
    );
    assert_eq!(saved.document.model().selected, b.selected);
    assert_eq!(second_base.raw_json(), second.load().unwrap().raw_json());

    a.workspaces[0].name = "First conflicting edit".into();
    assert!(!first.save(&mut first_base, &a).unwrap().conflict);
    b = saved.document.model().clone();
    b.workspaces[0].name = "Second conflicting edit".into();
    let committed = first.load().unwrap();
    let conflict = second.save(&mut second_base, &b).unwrap();
    assert!(conflict.conflict);
    assert_eq!(second_base.raw_json(), committed.raw_json());
    assert_eq!(second.load().unwrap().raw_json(), committed.raw_json());
    assert_eq!(conflict.document.model().selected, b.selected);
    b = conflict.document.model().clone();
    b.workspaces[0].name = "Explicit retry".into();
    let retry = second.save(&mut second_base, &b).unwrap();
    assert!(!retry.conflict);
    assert_eq!(retry.document.model().workspaces[0].name, "Explicit retry");
    let value: serde_json::Value = serde_json::from_str(second_base.raw_json()).unwrap();
    assert_eq!(value["extension"]["nullable"], serde_json::Value::Null);
    assert_eq!(
        value["extension"]["large"].to_string(),
        "123456789012345678901234567890"
    );
    assert!(!db.is_busy());
    assert_eq!(
        db.query_row("SELECT value FROM extension_row", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "retained"
    );
}

#[test]
fn writable_open_never_initializes_or_imports_and_save_rechecks_the_current_row() {
    let directory = tempfile::tempdir().unwrap();
    assert!(Store::open_existing_writable(directory.path()).is_err());
    assert!(!directory.path().join("layouts.db").exists());
    let legacy_db = Connection::open(directory.path().join("layouts.db")).unwrap();
    assert!(Store::open_existing_writable(directory.path()).is_err());
    assert_eq!(
        legacy_db
            .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let legacy = format!(" {RECORD} ");
    for table in ["layout", "shared_layout"] {
        legacy_db
            .execute_batch(&format!(
                "CREATE TABLE {table}(id INTEGER PRIMARY KEY, value TEXT)"
            ))
            .unwrap();
        legacy_db
            .execute(&format!("INSERT INTO {table} VALUES(1, ?1)"), [&legacy])
            .unwrap();
        assert!(Store::open_existing_writable(directory.path()).is_err());
    }
    assert_eq!(
        legacy_db
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='terminal_layout'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(legacy_db);
    let db = database(directory.path(), RECORD);
    let mut writer = Store::open_existing_writable(directory.path()).unwrap();
    let mut base = writer.load().unwrap();
    let original = base.raw_json().to_owned();
    let mut local = base.model().clone();
    local.workspaces[0].name = "Unsaved edit".into();
    for invalid in ["{}", "null"] {
        db.execute("UPDATE terminal_layout SET value=?1 WHERE id=1", [invalid])
            .unwrap();
        assert!(writer.save(&mut base, &local).is_err());
        assert!(Store::open_existing_writable(directory.path()).is_err());
        assert_eq!(base.raw_json(), original);
        assert_eq!(local.workspaces[0].name, "Unsaved edit");
        assert_eq!(
            db.query_row("SELECT value FROM terminal_layout WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            invalid
        );
    }
    db.execute("DELETE FROM terminal_layout", []).unwrap();
    assert!(writer.save(&mut base, &local).is_err());
    assert!(Store::open_existing_writable(directory.path()).is_err());
    db.execute("INSERT INTO terminal_layout VALUES(1, ?1)", [RECORD])
        .unwrap();
    db.execute_batch("ALTER TABLE terminal_layout ADD COLUMN extra TEXT DEFAULT 'retained'")
        .unwrap();
    assert!(writer.save(&mut base, &local).is_err());
    assert!(Store::open_existing_writable(directory.path()).is_err());
    for table in ["layout", "shared_layout"] {
        assert_eq!(
            db.query_row(&format!("SELECT value FROM {table}"), [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            legacy
        );
    }
    assert_eq!(base.raw_json(), original);
}

#[test]
fn failed_commit_preserves_base_and_unsaved_edits_and_releases_the_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let db = database(directory.path(), RECORD);
    let mut writer = Store::open_existing_writable(directory.path()).unwrap();
    let mut base = writer.load().unwrap();
    let mut local = base.model().clone();
    local.workspaces[0].name = "Retry after reader closes".into();
    db.execute_batch("BEGIN; SELECT value FROM terminal_layout")
        .unwrap();
    assert!(writer.save(&mut base, &local).is_err());
    assert_eq!(base.raw_json(), RECORD);
    assert_eq!(writer.load().unwrap().raw_json(), RECORD);
    db.execute_batch("ROLLBACK").unwrap();
    assert!(!writer.save(&mut base, &local).unwrap().conflict);
    assert_eq!(writer.load().unwrap().model(), &local);
    assert_eq!(
        db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
}

#[test]
fn oversized_save_preserves_the_record_base_and_unsaved_edit() {
    let directory = tempfile::tempdir().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(RECORD).unwrap();
    value["padding"] = "".into();
    value["padding"] = "x"
        .repeat(MAX_RECORD_BYTES - value.to_string().len() - 1)
        .into();
    let record = value.to_string();
    let db = database(directory.path(), &record);
    let mut writer = Store::open_existing_writable(directory.path()).unwrap();
    let mut base = writer.load().unwrap();
    let mut local = base.model().clone();
    local.workspaces[0].name = "x".repeat(80);
    assert!(
        writer
            .save(&mut base, &local)
            .unwrap_err()
            .detail
            .contains("16 MiB")
    );
    assert_eq!(base.raw_json(), record);
    assert_eq!(local.workspaces[0].name.len(), 80);
    assert_eq!(writer.load().unwrap().raw_json(), record);
    db.execute_batch("BEGIN EXCLUSIVE; ROLLBACK").unwrap();
}

#[test]
fn oversized_merge_rolls_back_without_losing_the_peer_commit_or_local_edits() {
    let directory = tempfile::tempdir().unwrap();
    let db = database(directory.path(), RECORD);
    let mut writer = Store::open_existing_writable(directory.path()).unwrap();
    let mut peer = Store::open_existing_writable(directory.path()).unwrap();
    let mut base = writer.load().unwrap();
    let mut peer_base = peer.load().unwrap();
    let mut local = base.model().clone();
    let mut other = peer_base.model().clone();
    local.workspaces[0].name = "a".repeat(9 * 1024 * 1024);
    other.workspaces[1].name = "b".repeat(9 * 1024 * 1024);
    assert!(base.with_model(&local, Path::new("layouts.db")).is_ok());
    assert!(!peer.save(&mut peer_base, &other).unwrap().conflict);
    assert!(
        writer
            .save(&mut base, &local)
            .unwrap_err()
            .detail
            .contains("16 MiB")
    );
    assert_eq!(base.raw_json(), RECORD);
    assert_eq!(local.workspaces[0].name.len(), 9 * 1024 * 1024);
    assert_eq!(writer.load().unwrap().raw_json(), peer_base.raw_json());
    db.execute_batch("BEGIN EXCLUSIVE; ROLLBACK").unwrap();
    local.workspaces[0].name = "Smaller retry".into();
    let saved = writer.save(&mut base, &local).unwrap();
    assert!(!saved.conflict);
    assert_eq!(saved.document.model().workspaces[0].name, "Smaller retry");
    assert_eq!(
        saved.document.model().workspaces[1].name,
        other.workspaces[1].name
    );
}

#[test]
fn schema_v2_keeps_order_attachments_navigation_and_original_json() {
    let document = parse(RECORD).unwrap();
    assert_eq!(document.raw_json(), RECORD);
    let layout = document.model();
    assert_eq!(layout.selected, "000000000001");
    // An absent preference shows the agents, as the previous tool did.
    assert!(layout.show_agents);
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
fn wal_reads_keep_committed_data_and_observe_later_writes() {
    let directory = tempfile::tempdir().unwrap();
    let db = database(directory.path(), RECORD);
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0")
        .unwrap();
    let changed = RECORD.replace("Development", "WAL commit");
    db.execute("UPDATE terminal_layout SET value=?1", [&changed])
        .unwrap();

    // The only writer is idle and stays open, so no write or checkpoint races
    // these copies. The separate fixture deliberately has no shared-memory file.
    let copy = tempfile::tempdir().unwrap();
    let names = ["layouts.db", "layouts.db-wal"];
    let before = names.map(|name| {
        let bytes = std::fs::read(directory.path().join(name)).unwrap();
        std::fs::write(copy.path().join(name), &bytes).unwrap();
        bytes
    });
    assert!(!copy.path().join("layouts.db-shm").exists());
    let copied = Store::open(copy.path()).unwrap();
    assert_eq!(copied.load().unwrap().raw_json(), changed);
    assert!(copy.path().join("layouts.db-shm").exists());
    drop(copied);
    for (name, expected) in names.into_iter().zip(before) {
        assert_eq!(std::fs::read(copy.path().join(name)).unwrap(), expected);
    }

    let store = Store::open(directory.path()).unwrap();
    let version = store.data_version().unwrap();
    assert_eq!(store.load().unwrap().raw_json(), changed);
    db.execute("UPDATE terminal_layout SET value=?1", [RECORD])
        .unwrap();
    assert_ne!(store.data_version().unwrap(), version);
    assert_eq!(store.load().unwrap().raw_json(), RECORD);
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
        if depth == 63 {
            let document = result.unwrap();
            let mut model = document.model().clone();
            model.rename_tab("At the depth limit");
            assert!(document.with_model(&model, Path::new("layouts.db")).is_ok());
            model.split(Direction::Right, "/tmp".into(), true);
            assert!(
                document
                    .with_model(&model, Path::new("layouts.db"))
                    .is_err()
            );
        }
    }
}

#[test]
fn model_edits_preserve_extensions_through_splits_moves_and_closes() {
    let record = RECORD
        .replace("\"direction\": \"right\"", "\"direction\": \"right\", \"empty\": {\"future\": true}")
        .replace("\"icon\": \"λ\"", "\"icon\": null, \"extra\": {\"tree\": 42}")
        .replace("\"name\": \"Editor\"", "\"name\": \"Editor\", \"extra\": 1.250e-1")
        .replace("\"agent\": \"build worker\"", "\"tabs\": [{\"id\": \"000000000002\", \"extra\": \"unrelated\"}], \"agent\": \"build worker\"");
    let document = parse(&record).unwrap();
    let path = Path::new("layouts.db");
    assert_eq!(
        document
            .with_model(document.model(), path)
            .unwrap()
            .raw_json(),
        record
    );
    let mut model = document.model().clone();
    assert!(model.rename_workspace("Renamed"));
    assert!(model.split(Direction::Below, "/tmp".into(), true));
    assert!(model.open_terminal());
    assert!(model.transfer_tab("000000000006"));
    let edited = document.with_model(&model, path).unwrap();
    assert_eq!(edited.model(), &model);
    assert_eq!(document.raw_json(), record);
    let before: serde_json::Value = serde_json::from_str(&record).unwrap();
    let after: serde_json::Value = serde_json::from_str(edited.raw_json()).unwrap();
    assert_eq!(after["extension"], before["extension"]);
    assert_eq!(
        after["workspaces"][0]["extra"],
        before["workspaces"][0]["extra"]
    );
    assert!(after["workspaces"][0]["icon"].is_null());
    let tab = &after["workspaces"][1]["tabs"][0];
    assert_eq!(tab["extra"], before["workspaces"][0]["tabs"][0]["extra"]);
    assert!(!after.as_object().unwrap().contains_key("show_agents"));
    assert!(!tab["tree"].as_object().unwrap().contains_key("ratio"));
    assert_eq!(
        tab["tree"]["empty"],
        before["workspaces"][0]["tabs"][0]["tree"]["empty"]
    );
    assert_eq!(tab["tree"]["first"]["ratio"], 0.5);
    assert_eq!(
        tab["tree"]["first"]["first"],
        before["workspaces"][0]["tabs"][0]["tree"]["first"]
    );
    assert!(
        !tab["tree"]["first"]["second"]
            .as_object()
            .unwrap()
            .contains_key("empty")
    );

    assert!(model.close_pane());
    assert!(model.attach(None));
    let collapsed = edited.with_model(&model, path).unwrap();
    let after: serde_json::Value = serde_json::from_str(collapsed.raw_json()).unwrap();
    let leaf = &after["workspaces"][1]["tabs"][0]["tree"]["first"];
    assert_eq!(leaf["id"], "000000000004");
    assert_eq!(
        leaf["tabs"],
        before["workspaces"][0]["tabs"][0]["tree"]["first"]["tabs"]
    );
    assert!(leaf["agent"].is_null());
    assert!(!leaf.as_object().unwrap().contains_key("source_socket"));
    assert_eq!(collapsed.model(), &model);
    let explicit = parse(
        &RECORD
            .replace(
                "\"direction\": \"right\"",
                "\"direction\": \"right\", \"ratio\": 5E-1",
            )
            .replace(
                "\"nullable\": null",
                "\"nullable\": null, \"exponent\": 1e2",
            ),
    )
    .unwrap();
    let mut model = explicit.model().clone();
    model.rename_tab("Keep ratio spelling");
    let projected = explicit.with_model(&model, path).unwrap();
    assert!(projected.raw_json().contains("5E-1"));
    assert!(projected.raw_json().contains("1e2"));
    let boundary = parse(&RECORD.replace(
        "\"direction\": \"right\"",
        "\"direction\": \"right\", \"ratio\": 0.5000000000000000555",
    ))
    .unwrap();
    let mut model = boundary.model().clone();
    model.rename_tab("Keep boundary ratio");
    assert!(
        boundary
            .with_model(&model, path)
            .unwrap()
            .raw_json()
            .contains("0.5000000000000000555")
    );
    let Node::Split { ratio, .. } = &mut model.workspaces[0].tabs[0].tree else {
        panic!()
    };
    *ratio = 0.5000000000000001;
    assert_eq!(boundary.with_model(&model, path).unwrap().model(), &model);
}

#[test]
fn model_projection_rejects_invalid_edits_without_changing_the_document() {
    let document = parse(RECORD).unwrap();
    let path = Path::new("layouts.db");
    let mut model = document.model().clone();
    model.workspaces[1].id = model.workspaces[0].id.clone();
    assert!(document.with_model(&model, path).is_err());
    let mut model = document.model().clone();
    let Node::Split { ratio, .. } = &mut model.workspaces[0].tabs[0].tree else {
        panic!()
    };
    *ratio = f64::NAN;
    assert!(document.with_model(&model, path).is_err());
    let mut model = document.model().clone();
    model.workspaces[0].name = "x".repeat(MAX_RECORD_BYTES);
    assert!(document.with_model(&model, path).is_err());
    assert_eq!(document.raw_json(), RECORD);
}

#[test]
fn reconciliation_merges_refreshes_conflicts_and_retains_local_navigation() {
    let base = parse(RECORD).unwrap();
    let path = Path::new("layouts.db");
    let mut local = base.model().clone();
    local.rename_tab("Local tab");
    local.choose_pane("000000000005");
    local.focus = true;
    let mut peer = base.model().clone();
    peer.rename_workspace("Peer workspace");
    peer.choose_workspace("000000000006");
    peer.show_agents = true;
    let latest = base.with_model(&peer, path).unwrap();
    let merged = base.reconcile(&local, &latest, path).unwrap();
    assert!(!merged.conflict && !merged.active_tree_changed);
    let model = merged.document.model();
    assert_eq!(model.workspace().unwrap().name, "Peer workspace");
    assert_eq!(model.tab().unwrap().name, "Local tab");
    assert_eq!(model.pane().unwrap().id, "000000000005");
    assert!(model.focus && model.show_agents);
    assert_eq!(model.selected, local.selected);
    // Advance the base to the last incoming document, keeping unsaved local edits.
    peer.choose_workspace("000000000001");
    peer.rename_workspace("Peer again");
    let newer = latest.with_model(&peer, path).unwrap();
    let next = latest.reconcile(model, &newer, path).unwrap();
    assert!(!next.conflict);
    assert_eq!(next.document.model().tab().unwrap().name, "Local tab");
    assert_eq!(
        next.document.model().workspace().unwrap().name,
        "Peer again"
    );

    peer.rename_tab("Peer tab");
    let conflicting = latest.with_model(&peer, path).unwrap();
    let refreshed = base.reconcile(&local, &conflicting, path).unwrap();
    assert!(refreshed.conflict && !refreshed.active_tree_changed);
    assert_eq!(refreshed.document.model().tab().unwrap().name, "Peer tab");
    assert_eq!(
        refreshed.document.model().pane().unwrap().id,
        "000000000005"
    );
    assert_eq!(local.tab().unwrap().name, "Local tab");

    let mut peer = base.model().clone();
    peer.attach(None);
    let changed = base.with_model(&peer, path).unwrap();
    let changed = base.reconcile(&local, &changed, path).unwrap();
    assert!(!changed.conflict && changed.active_tree_changed);
    assert_eq!(changed.document.model().tab().unwrap().name, "Local tab");
    peer.close_tab();
    let deleted = base.with_model(&peer, path).unwrap();
    let refreshed = base.reconcile(&local, &deleted, path).unwrap();
    assert!(refreshed.conflict && refreshed.active_tree_changed);
    assert!(refreshed.document.model().tab().is_none());
    assert_eq!(base.raw_json(), RECORD);
}

#[test]
fn reconciliation_refreshes_invalid_combinations_but_preserves_oversized_edits() {
    let base = parse(RECORD).unwrap();
    let path = Path::new("layouts.db");
    let add = |model: &mut tmux_workspaces_core::Layout| {
        model.add_tab(None, "/tmp".into(), true);
        let tab = model.workspaces[0].tabs.last_mut().unwrap();
        let Node::Leaf(leaf) = &mut tab.tree else {
            panic!()
        };
        leaf.id = "aaaaaaaaaaaa".into();
        tab.focus = leaf.id.clone();
    };
    let mut local = base.model().clone();
    let mut peer = base.model().clone();
    add(&mut local);
    add(&mut peer);
    let latest = base.with_model(&peer, path).unwrap();
    // Each side is valid, but their distinct added tabs share a leaf identity.
    let refreshed = base.reconcile(&local, &latest, path).unwrap();
    assert!(refreshed.conflict);
    assert_eq!(refreshed.document.model().workspaces[0].tabs.len(), 2);

    let mut local = base.model().clone();
    let mut peer = base.model().clone();
    local.workspaces[0].name = "a".repeat(9 * 1024 * 1024);
    peer.workspaces[1].name = "b".repeat(9 * 1024 * 1024);
    let latest = base.with_model(&peer, path).unwrap();
    assert!(base.reconcile(&local, &latest, path).is_err());
    assert_eq!(local.workspaces[0].name.len(), 9 * 1024 * 1024);
    assert_eq!(latest.model(), &peer);
    assert_eq!(base.raw_json(), RECORD);
}

#[test]
fn a_native_icon_name_round_trips_and_clearing_it_shows_the_number() {
    let path = Path::new("layouts.db");
    let document =
        parse(&RECORD.replace("\"icon\": \"λ\"", "\"icon\": \"λ\", \"symbol\": \"rocket\""))
            .unwrap();
    assert_eq!(
        document.model().workspaces[0].symbol.as_deref(),
        Some("rocket")
    );
    assert!(parse(&RECORD.replace("\"icon\": \"λ\"", "\"symbol\": \"Rocket!\"")).is_err());
    let mut model = document.model().clone();
    assert!(model.set_workspace_symbol(Some("code")));
    let saved: serde_json::Value =
        serde_json::from_str(document.with_model(&model, path).unwrap().raw_json()).unwrap();
    assert_eq!(saved["workspaces"][0]["symbol"], "code");
    assert_eq!(saved["workspaces"][0]["icon"], "λ");
    assert!(model.set_workspace_symbol(None));
    assert!(!model.set_workspace_symbol(None));
    let cleared: serde_json::Value =
        serde_json::from_str(document.with_model(&model, path).unwrap().raw_json()).unwrap();
    assert!(cleared["workspaces"][0]["symbol"].is_null());
    assert!(cleared["workspaces"][0]["icon"].is_null());
}

#[test]
fn view_panes_round_trip_and_reject_mixed_or_unknown_views() {
    let view = RECORD.replacen(
        "\"agent\": null, \"empty\": true",
        "\"agent\": null, \"view\": \"agent\", \"view_agent\": \"worker-1\"",
        1,
    );
    let document = parse(&view).unwrap();
    let Node::Split { second, .. } = &document.model().workspaces[0].tabs[0].tree else {
        panic!("expected a split");
    };
    let Node::Leaf(leaf) = second.as_ref() else {
        panic!("expected a leaf");
    };
    let expected = View::agent("worker-1".into(), None);
    assert_eq!(leaf.view.as_ref(), Some(&expected));
    assert!(!leaf.runs_terminal());

    let mut model = document.model().clone();
    for (kind, name) in [
        (ViewKind::Usage, "usage"),
        (ViewKind::Reports, "reports"),
        (ViewKind::Artifacts, "artifacts"),
    ] {
        let Node::Split { second, .. } = &mut model.workspaces[0].tabs[0].tree else {
            unreachable!();
        };
        let Node::Leaf(leaf) = second.as_mut() else {
            unreachable!();
        };
        leaf.view = Some(View::of(kind));
        let saved = document
            .with_model(&model, Path::new("layouts.db"))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(saved.raw_json()).unwrap();
        let pane = &value["workspaces"][0]["tabs"][0]["tree"]["second"];
        assert_eq!(pane["view"], name);
        assert!(pane.get("view_agent").is_none());
        assert_eq!(saved.model(), &model);
        assert_eq!(parse(saved.raw_json()).unwrap().model(), &model);
    }

    for (before, after) in [
        ("\"view\": \"agent\"", "\"view\": \"future\""),
        ("\"view\": \"agent\"", "\"view\": \"usage\""),
        (
            "\"view_agent\": \"worker-1\"",
            "\"view_agent\": \"bad name\"",
        ),
        ("\"agent\": null, \"view\"", "\"agent\": \"work\", \"view\""),
        (
            "\"view\": \"agent\"",
            "\"view\": \"agent\", \"view_section\": \"later\"",
        ),
        (
            "\"view\": \"agent\", \"view_agent\": \"worker-1\"",
            "\"view\": \"usage\", \"view_section\": \"usage\"",
        ),
        (
            "\"view\": \"agent\", \"view_agent\": \"worker-1\"",
            "\"view_section\": \"usage\"",
        ),
        (
            "\"agent\": null, \"view\"",
            "\"agent\": null, \"empty\": true, \"view\"",
        ),
    ] {
        let text = view.replacen(before, after, 1);
        assert_ne!(text, view);
        assert!(parse(&text).is_err(), "accepted {after}");
    }
    let stray = RECORD.replacen(
        "\"direction\": \"right\"",
        "\"direction\": \"right\", \"view\": \"usage\"",
        1,
    );
    assert!(parse(&stray).is_err());
}

// Before the agent view had sections, one agent's policies were a view of
// their own. Such a pane opens on the agent's Policies section, and is saved
// that way with the next change; without an agent, the saved name is the team-wide library.
#[test]
fn saved_agent_policies_open_on_the_agent_section_and_libraries_stay() {
    let parse_pane =
        |text: &str| parse(&RECORD.replacen("\"agent\": null, \"empty\": true", text, 1)).unwrap();
    let view = |document: &Document| {
        let Node::Split { second, .. } = &document.model().workspaces[0].tabs[0].tree else {
            panic!("expected a split");
        };
        let Node::Leaf(leaf) = second.as_ref() else {
            panic!("expected a leaf");
        };
        leaf.view.clone()
    };
    let document =
        parse_pane("\"agent\": null, \"view\": \"policies\", \"view_agent\": \"worker-1\"");
    let section = View::agent("worker-1".into(), Some(AgentSection::Policies));
    assert_eq!(view(&document), Some(section));
    // The new form is written with the next change.
    let mut model = document.model().clone();
    model.workspaces[0].name = "renamed".into();
    let saved = document
        .with_model(&model, Path::new("layouts.db"))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(saved.raw_json()).unwrap();
    let written = &value["workspaces"][0]["tabs"][0]["tree"]["second"];
    assert_eq!(written["view"], "agent");
    assert_eq!(written["view_agent"], "worker-1");
    assert_eq!(written["view_section"], "policies");

    for kind in [ViewKind::Skills, ViewKind::Policies, ViewKind::Tags] {
        let text = format!("\"agent\": null, \"view\": \"{}\"", kind.as_str());
        assert_eq!(view(&parse_pane(&text)), Some(View::of(kind)));
    }
}
