use super::*;
use crate::profiles::Profile;

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn create() -> Prepared {
    Prepared::create("operation-create-a", "Work", "baseline-example-v1").unwrap()
}
fn restored(prepared: &Prepared) -> Prepared {
    let checkpoint = prepared.checkpoint().unwrap();
    let restored = Prepared::restore(&checkpoint).unwrap();
    assert_eq!(restored.bytes(), prepared.bytes());
    assert_eq!(restored.identity(), prepared.identity());
    assert_eq!(restored.checkpoint().unwrap(), checkpoint);
    restored
}

#[test]
fn all_actions_round_trip_original_bytes_without_discovery() {
    let fixture = fixture();
    let profile = Profile::parse(&bytes(&fixture["discovery"]["show_ready"])).unwrap();
    for prepared in [
        create(),
        Prepared::start("operation-start-a", &profile).unwrap(),
        Prepared::stop("operation-stop-a", &profile).unwrap(),
    ] {
        restored(&prepared);
    }
}

#[test]
fn recovery_retains_first_reservation_and_terminal_nonregression() {
    let fixture = fixture();
    let lifecycle = &fixture["lifecycle"];
    let mut prepared = restored(&create());
    prepared
        .observe(&bytes(&lifecycle["create_pending"]))
        .unwrap();
    let mut prepared = restored(&prepared);
    prepared
        .observe(&bytes(&lifecycle["create_reserved_pending"]))
        .unwrap();
    let mut prepared = restored(&prepared);
    assert_eq!(
        prepared.observe(&bytes(&lifecycle["create_pending"])),
        Err(Error::BindingMismatch)
    );
    let mut replaced = lifecycle["create_reserved_pending"].clone();
    replaced["lifecycle_id"] = json!("replacement");
    assert_eq!(
        prepared.observe(&bytes(&replaced)),
        Err(Error::BindingMismatch)
    );
    let mut success = lifecycle["create_reserved_pending"].clone();
    success["status"] = json!("succeeded");
    let terminal = prepared.observe(&bytes(&success)).unwrap();
    let mut prepared = restored(&prepared);
    assert_eq!(prepared.observe(&bytes(&success)).unwrap(), terminal);
    assert_eq!(
        prepared.observe(&bytes(&lifecycle["create_reserved_pending"])),
        Err(Error::Invalid)
    );
}

#[test]
fn corrupt_checkpoints_do_not_normalize_or_replace_requests() {
    let original: Value = serde_json::from_slice(&create().checkpoint().unwrap()).unwrap();
    for request in [
        "{}".to_owned(),
        format!(" {}", original["request"].as_str().unwrap()),
        original["request"]
            .as_str()
            .unwrap()
            .replace("\"action\":\"create\"", "\"action\":\"stop\""),
        original["request"]
            .as_str()
            .unwrap()
            .replace("\"schema_version\":1", "\"schema_version\":2"),
        original["request"]
            .as_str()
            .unwrap()
            .replace("{", "{\"extra\":true,"),
        original["request"]
            .as_str()
            .unwrap()
            .replace("{", "{\"request_id\":\"other\","),
    ] {
        let mut invalid = original.clone();
        invalid["request"] = json!(request);
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    for (field, value) in [
        ("checkpoint_version", json!(2)),
        ("extra", json!(true)),
        ("observation", fixture()["lifecycle"]["succeeded"].clone()),
        ("request", json!("x".repeat(crate::RESPONSE_LIMIT + 1))),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    assert!(Prepared::restore(&vec![b' '; CHECKPOINT_LIMIT + 1]).is_err());
    assert!(Prepared::restore(&bytes(&json!([1, original["request"], null]))).is_err());
}

#[test]
fn boundary_sized_requests_and_sanitized_receipts_remain_restorable() {
    let fixture = fixture();
    // Lookup IDs have a smaller independent bound. Keep exercising the exact
    // wire boundary through request content and retained receipt metadata.
    let overhead = Prepared::create("x", "x", "baseline")
        .unwrap()
        .bytes()
        .len()
        - 1;
    let name = "x".repeat(crate::RESPONSE_LIMIT - overhead);
    let prepared = Prepared::create("x", &name, "baseline").unwrap();
    assert_eq!(prepared.bytes().len(), crate::RESPONSE_LIMIT);
    restored(&prepared);

    let mut failed = fixture["lifecycle"]["failed"].clone();
    failed["error"]["message"] = json!("x");
    failed["request_id"] = json!("x");
    failed["action"] = json!("create");
    failed["profile_id"] = json!("x");
    let overhead = bytes(&failed).len() - 1;
    failed["profile_id"] = json!("x".repeat(crate::RESPONSE_LIMIT - overhead));
    assert_eq!(bytes(&failed).len(), crate::RESPONSE_LIMIT);
    let mut prepared = Prepared::create("x", "Work", "baseline").unwrap();
    let terminal = prepared.observe(&bytes(&failed)).unwrap();
    let mut prepared = restored(&prepared);
    assert_eq!(prepared.observe(&bytes(&failed)).unwrap(), terminal);
    let mut oversized = failed;
    oversized["error"]["message"] = json!("xx");
    assert_eq!(prepared.observe(&bytes(&oversized)), Err(Error::Invalid));
}
