use super::*;
use crate::profiles::operations::tests::served;

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn create() -> Prepared {
    Prepared::create("operation-create-a", "Work").unwrap()
}
fn restored(prepared: &Prepared) -> Prepared {
    let checkpoint = prepared.checkpoint().unwrap();
    let restored = Prepared::restore(&checkpoint).unwrap();
    assert_eq!(restored.bytes(), prepared.bytes());
    assert_eq!(restored.identity(), prepared.identity());
    assert_eq!(restored.terminal(), prepared.terminal());
    assert_eq!(restored.checkpoint().unwrap(), checkpoint);
    restored
}

#[test]
fn requests_and_their_results_round_trip_without_discovery() {
    restored(&create());
    let mut made = create();
    made.observe_created(&served()).unwrap();
    let mut made = restored(&made);
    // A restored result still cannot change.
    assert_eq!(
        made.observe_refused("unavailable").err(),
        Some(Error::BindingMismatch)
    );
    let mut refused = create();
    refused.observe_refused("unavailable").unwrap();
    restored(&refused);
}

#[test]
fn corrupt_or_unknown_checkpoints_do_not_normalize_or_replace_requests() {
    let original: Value = serde_json::from_slice(&create().checkpoint().unwrap()).unwrap();
    let request = original["request"].as_str().unwrap();
    for request in [
        "{}".to_owned(),
        format!(" {request}"),
        request.replace("\"action\":\"create\"", "\"action\":\"stop\""),
        request.replace('{', "{\"extra\":true,"),
        request.replace('{', "{\"request_id\":\"other\","),
    ] {
        let mut invalid = original.clone();
        invalid["request"] = json!(request);
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    let identity = json!({"profile_id": "a", "installation_id": "b", "lifecycle_id": "c"});
    for (field, value) in [
        ("checkpoint_version", json!(99)),
        ("extra", json!(true)),
        (
            "observation",
            json!({"identity": null, "status": "succeeded", "error_code": null}),
        ),
        (
            "observation",
            json!({"identity": identity, "status": "failed", "error_code": "refused"}),
        ),
        (
            "observation",
            json!({"identity": identity, "status": "pending", "error_code": null}),
        ),
        ("request", json!("x".repeat(crate::RESPONSE_LIMIT + 1))),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    // A record of a checkpoint version this app doesn't read is not replayed.
    let unknown = json!({"checkpoint_version": 99, "observation": null,
        "request": "{\"action\":\"create\",\"display_name\":\"Work\",\"request_id\":\"a\"}"});
    assert!(matches!(
        Prepared::restore(&bytes(&unknown)),
        Err(Error::Invalid)
    ));
    assert!(Prepared::restore(&vec![b' '; CHECKPOINT_LIMIT + 1]).is_err());
    assert!(Prepared::restore(&bytes(&json!([1, original["request"], null]))).is_err());
}

#[test]
fn boundary_sized_requests_remain_restorable() {
    let overhead = Prepared::create("x", "x").unwrap().bytes().len() - 1;
    let name = "x".repeat(crate::RESPONSE_LIMIT - overhead);
    let prepared = Prepared::create("x", &name).unwrap();
    assert_eq!(prepared.bytes().len(), crate::RESPONSE_LIMIT);
    restored(&prepared);
}
