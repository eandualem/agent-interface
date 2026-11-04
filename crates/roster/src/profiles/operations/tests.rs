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
fn lifecycle_intent_and_recovery_refuse_oversized_lookup_ids() {
    let profile = profile();
    let preview = AdoptionPreview::parse(&bytes(&fixture()["adoption"]["preview"])).unwrap();
    for length in [REQUEST_ID_LIMIT + 1, crate::RESPONSE_LIMIT - 1024] {
        let id = "x".repeat(length);
        for result in [
            Prepared::create(&id, "Work", "baseline"),
            Prepared::start(&id, &profile),
            Prepared::stop(&id, &profile),
            Prepared::adopt_default(&id, &preview),
        ] {
            assert!(matches!(result, Err(Error::Invalid)));
        }
        let request = json!({"schema_version":1,"request_id":id,"action":"create",
            "display_name":"Work","baseline_version":"baseline"});
        let checkpoint = json!({"checkpoint_version":1,
            "request":serde_json::to_string(&request).unwrap(),"observation":null});
        assert!(matches!(
            Prepared::restore(&bytes(&checkpoint)),
            Err(Error::Invalid)
        ));
    }
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
fn malformed_failure_cannot_reserve_identity_or_establish_terminal_progress() {
    let lifecycle = &fixture()["lifecycle"];
    let mut create = Prepared::create("operation-create-a", "Work", "baseline-example-v1").unwrap();
    let original = create.bytes().to_vec();
    let mut failed = lifecycle["create_reserved_pending"].clone();
    failed["status"] = json!("failed");
    failed["error"] = json!(["binding_mismatch", "failure"]);
    assert_eq!(create.observe(&bytes(&failed)), Err(Error::Invalid));
    assert!(create.identity().is_none());
    assert_eq!(create.bytes(), original);
    let mut success = lifecycle["create_reserved_pending"].clone();
    success["status"] = json!("succeeded");
    assert_eq!(
        create.observe(&bytes(&success)).unwrap().status,
        Status::Succeeded
    );
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

#[test]
fn adoption_keeps_preview_token_and_pins_the_first_reserved_identity() {
    let adoption = &fixture()["adoption"];
    let preview = AdoptionPreview::parse(&bytes(&adoption["preview"])).unwrap();
    assert_eq!(preview.installation_id(), "installation-default");
    assert_eq!(
        preview.data_directory(),
        std::path::Path::new("/example/default")
    );
    assert_eq!(
        preview.tmux_socket(),
        std::path::Path::new("/example/default/tmux.sock")
    );
    assert_eq!(preview.baseline_version(), "baseline-example-v1");
    let mut prepared = Prepared::adopt_default("adopt-default-1", &preview).unwrap();
    let original = prepared.bytes().to_vec();
    assert_eq!(
        serde_json::from_slice::<Value>(&original).unwrap(),
        adoption["request"]
    );
    assert!(
        prepared
            .observe(&bytes(&adoption["pending"]))
            .unwrap()
            .identity
            .is_none()
    );
    let mut reserved = adoption["succeeded"].clone();
    reserved["status"] = json!("pending");
    let identity = prepared
        .observe(&bytes(&reserved))
        .unwrap()
        .identity
        .unwrap();
    for field in ["profile_id", "installation_id", "lifecycle_id"] {
        let mut changed = reserved.clone();
        changed[field] = json!("replacement");
        assert_eq!(
            prepared.observe(&bytes(&changed)),
            Err(Error::BindingMismatch)
        );
        assert_eq!(prepared.identity(), Some(&identity));
    }
    assert!(prepared.observe(&bytes(&adoption["pending"])).is_err());
    let terminal = prepared.observe(&bytes(&adoption["succeeded"])).unwrap();
    let mut refreshed = adoption["preview"].clone();
    refreshed["adoption_token"] = json!("new-token");
    let refreshed = AdoptionPreview::parse(&bytes(&refreshed)).unwrap();
    let next = Prepared::adopt_default("adopt-default-2", &refreshed).unwrap();
    assert_ne!(next.bytes(), original);
    assert_eq!(prepared.bytes(), original);
    assert_eq!(
        prepared.observe(&bytes(&adoption["succeeded"])).unwrap(),
        terminal
    );
    assert!(prepared.observe(&bytes(&adoption["changed"])).is_err());
}

#[test]
fn adoption_rejects_foreign_installation_before_reservation_and_stale_token_failure_is_terminal() {
    let adoption = &fixture()["adoption"];
    let preview = AdoptionPreview::parse(&bytes(&adoption["preview"])).unwrap();
    let mut prepared = Prepared::adopt_default("adopt-default-1", &preview).unwrap();
    let mut foreign = adoption["succeeded"].clone();
    foreign["installation_id"] = json!("another-installation");
    assert_eq!(
        prepared.observe(&bytes(&foreign)),
        Err(Error::BindingMismatch)
    );
    assert!(prepared.identity().is_none());
    let failed = prepared.observe(&bytes(&adoption["changed"])).unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("adoption_changed"));
    assert!(failed.identity.is_none());
    assert_eq!(
        prepared.observe(&bytes(&adoption["changed"])).unwrap(),
        failed
    );
    assert!(prepared.observe(&bytes(&adoption["succeeded"])).is_err());
    let mut repeat = Prepared::adopt_default("adopt-default-2", &preview).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(repeat.bytes()).unwrap(),
        adoption["repeat_request"]
    );
    let result = repeat
        .observe(&bytes(&adoption["repeat_succeeded"]))
        .unwrap();
    assert_eq!(result.identity.unwrap().profile_id, "profile-default");
}

#[test]
fn adoption_preview_rejects_malformed_or_unsupported_metadata_without_io() {
    let canonical = fixture()["adoption"]["preview"].clone();
    for (field, value) in [
        ("installation_id", json!("")),
        ("baseline_version", json!("bad version")),
        ("adoption_token", json!(null)),
        ("data_dir", json!("relative")),
        ("data_dir", json!("/bad\npath")),
        ("tmux", json!({"socket_path":"relative"})),
        ("tmux", json!(["/example/default/tmux.sock"])),
        ("schema_version", json!(2)),
    ] {
        let mut invalid = canonical.clone();
        invalid[field] = value;
        assert!(AdoptionPreview::parse(&bytes(&invalid)).is_err());
    }
    for field in [
        "installation_id",
        "baseline_version",
        "adoption_token",
        "data_dir",
        "tmux",
    ] {
        let mut invalid = canonical.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(AdoptionPreview::parse(&bytes(&invalid)).is_err());
    }
    let preview = AdoptionPreview::parse(&bytes(&canonical)).unwrap();
    assert!(Prepared::adopt_default("", &preview).is_err());
}
