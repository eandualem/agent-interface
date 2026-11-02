use super::*;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn profile() -> Profile {
    Profile::parse(&serde_json::to_vec(&fixture()["discovery"]["show_ready"]).unwrap()).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn starting() -> Prepared {
    Prepared::start("operation-start-a", &profile()).unwrap()
}

#[test]
fn prepared_requests_match_canonical_bodies_and_keep_exact_retry_bytes() {
    let fixture = fixture();
    let lifecycle = &fixture["lifecycle"];
    let create = Prepared::create("operation-create-a", "Work", "baseline-example-v1").unwrap();
    let mut start = starting();
    let stop = Prepared::stop("operation-stop-a", &profile()).unwrap();
    for (prepared, key) in [(&create, "create"), (&start, "start"), (&stop, "stop")] {
        assert_eq!(
            serde_json::from_slice::<Value>(prepared.bytes()).unwrap(),
            lifecycle[key]
        );
    }
    let original = start.bytes().to_vec();
    assert_eq!(start.request_id(), "operation-start-a");
    assert_eq!(
        start.observe(&bytes(&lifecycle["pending"])).unwrap().status,
        Status::Pending
    );
    let terminal = start.observe(&bytes(&lifecycle["succeeded"])).unwrap();
    assert_eq!(terminal.status, Status::Succeeded);
    assert_eq!(
        start.observe(&bytes(&lifecycle["succeeded"])).unwrap(),
        terminal
    );
    assert_eq!(
        start.observe(&bytes(&lifecycle["pending"])),
        Err(Error::Invalid)
    );
    assert_eq!(
        start.observe(&bytes(&lifecycle["failed"])),
        Err(Error::Invalid)
    );
    assert_eq!(start.bytes(), original);
}

#[test]
fn create_pins_the_first_reserved_tuple_without_regressing_or_rebinding() {
    let lifecycle = &fixture()["lifecycle"];
    let mut create = Prepared::create("operation-create-a", "Work", "baseline-example-v1").unwrap();
    let original = create.bytes().to_vec();
    assert!(
        create
            .observe(&bytes(&lifecycle["create_pending"]))
            .unwrap()
            .identity
            .is_none()
    );
    let reserved = create
        .observe(&bytes(&lifecycle["create_reserved_pending"]))
        .unwrap()
        .identity
        .unwrap();
    assert_eq!(create.identity(), Some(&reserved));
    for field in ["profile_id", "installation_id", "lifecycle_id"] {
        let mut changed = lifecycle["create_reserved_pending"].clone();
        changed[field] = json!("other");
        assert_eq!(
            create.observe(&bytes(&changed)),
            Err(Error::BindingMismatch)
        );
        assert_eq!(create.identity(), Some(&reserved));
    }
    assert_eq!(
        create.observe(&bytes(&lifecycle["create_pending"])),
        Err(Error::BindingMismatch)
    );
    let mut success = lifecycle["create_reserved_pending"].clone();
    success["status"] = json!("succeeded");
    assert_eq!(
        create.observe(&bytes(&success)).unwrap().status,
        Status::Succeeded
    );
    assert_eq!(create.bytes(), original);
}

#[test]
fn malformed_or_foreign_receipts_cannot_change_retained_progress() {
    let lifecycle = &fixture()["lifecycle"];
    let mut invalid = Vec::new();
    for (field, value) in [
        ("request_id", json!("different-request")),
        ("action", json!("stop")),
        ("action", json!("adopt")),
        ("schema_version", json!(99)),
        ("profile_id", Value::Null),
        ("installation_id", json!("other")),
        ("lifecycle_id", json!("replacement")),
        ("error", json!({"code":"unexpected","message":"failed"})),
    ] {
        let mut receipt = lifecycle["pending"].clone();
        receipt[field] = value;
        invalid.push(receipt);
    }
    for field in [
        "profile_id",
        "installation_id",
        "lifecycle_id",
        "error",
        "status",
    ] {
        let mut receipt = lifecycle["pending"].clone();
        receipt.as_object_mut().unwrap().remove(field);
        invalid.push(receipt);
    }
    invalid.push(fixture()["operation_lookup_unknown"]["body"].clone());
    for receipt in invalid {
        let mut start = starting();
        let original = start.bytes().to_vec();
        let origin = start.identity().cloned();
        assert!(
            start.observe(&bytes(&receipt)).is_err(),
            "accepted {receipt}"
        );
        assert_eq!(start.bytes(), original);
        assert_eq!(start.identity(), origin.as_ref());
        assert_eq!(
            start
                .observe(&bytes(&lifecycle["succeeded"]))
                .unwrap()
                .status,
            Status::Succeeded
        );
    }
}

#[test]
fn failed_create_before_reservation_is_terminal_and_drops_provider_prose() {
    let mut create = Prepared::create("operation-create-a", "Work", "baseline-example-v1").unwrap();
    let mut failed = fixture()["lifecycle"]["create_pending"].clone();
    failed["status"] = json!("failed");
    failed["error"] =
        json!({"code":"creation_failed","message":"private provider diagnostic prose"});
    let receipt = create.observe(&bytes(&failed)).unwrap();
    assert!(receipt.identity.is_none());
    assert_eq!(receipt.error_code.as_deref(), Some("creation_failed"));
    assert!(!format!("{receipt:?}").contains("private provider diagnostic prose"));
    assert_eq!(create.observe(&bytes(&failed)).unwrap(), receipt);
    assert!(
        create
            .observe(&bytes(&fixture()["lifecycle"]["create_reserved_pending"]))
            .is_err()
    );
    let mut start = starting();
    let mut missing = fixture()["lifecycle"]["failed"].clone();
    missing["error"] = Value::Null;
    assert!(start.observe(&bytes(&missing)).is_err());
    missing["error"] = json!({"code":"x".repeat(129),"message":"failure"});
    assert!(start.observe(&bytes(&missing)).is_err());
}

#[test]
fn requests_reject_nonready_profiles_and_invalid_or_oversized_input() {
    let creating = Profile::parse(&bytes(&fixture()["discovery"]["show_creating"])).unwrap();
    assert!(matches!(
        Prepared::start("request", &creating),
        Err(Error::NotReady)
    ));
    assert!(matches!(
        Prepared::stop("request", &creating),
        Err(Error::NotReady)
    ));
    assert!(matches!(
        Prepared::start("two words", &profile()),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Prepared::create("request", "  ", "baseline"),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Prepared::create("request", "Work", "bad baseline"),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Prepared::create("request", &"x".repeat(crate::RESPONSE_LIMIT), "baseline"),
        Err(Error::Invalid)
    ));
}
