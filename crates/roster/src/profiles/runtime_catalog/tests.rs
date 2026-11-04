use super::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn identity() -> Identity {
    Identity {
        profile_id: "profile-a".into(),
        installation_id: "installation-a".into(),
        lifecycle_id: "lifecycle-a1".into(),
    }
}
fn parse(value: &Value) -> Result<Catalog, Error> {
    Catalog::parse(&serde_json::to_vec(value).unwrap(), &identity())
}

#[test]
fn canonical_observations_keep_authentication_separate_from_compatibility() {
    let fixture = fixture();
    let mut wire = fixture["runtime_catalog"]["catalog"].clone();
    for (status, expected) in [
        ("not_authenticated", Authentication::NotAuthenticated),
        ("unavailable", Authentication::Unavailable),
        ("unsupported", Authentication::Unsupported),
        ("unknown", Authentication::Unknown),
        ("future_status", Authentication::Unknown),
    ] {
        wire["items"][0]["authentication"] = json!(status);
        let catalog = parse(&wire).unwrap();
        assert_eq!(catalog.identity(), &identity());
        assert!(catalog.items()[0].setup_compatibility.compatible);
        assert_eq!(catalog.items()[0].authentication, expected);
        // Provider paths and unattended arguments are not available for execution.
        assert!(!format!("{catalog:?}").contains("/example/bin"));
        assert!(!format!("{catalog:?}").contains("workspace-write"));
    }
    assert!(
        parse(&fixture["runtime_catalog"]["empty"])
            .unwrap()
            .items()
            .is_empty()
    );
}

#[test]
fn catalogs_reject_other_instances_malformed_rows_and_legacy_schema() {
    let original = fixture()["runtime_catalog"]["catalog"].clone();
    for field in ["profile_id", "installation_id", "lifecycle_id"] {
        let mut wrong = original.clone();
        wrong[field] = json!("other");
        assert_eq!(parse(&wrong).unwrap_err(), Error::BindingMismatch);
    }
    let mut wrong = original.clone();
    wrong["schema_version"] = json!(1);
    assert_eq!(parse(&wrong).unwrap_err(), Error::Unsupported);
    for field in ["authentication", "setup_compatibility", "available"] {
        let mut wrong = original.clone();
        wrong["items"][0].as_object_mut().unwrap().remove(field);
        assert!(parse(&wrong).is_err());
    }
    let mut duplicate = original.clone();
    duplicate["items"]
        .as_array_mut()
        .unwrap()
        .push(original["items"][0].clone());
    assert!(parse(&duplicate).is_err());
    let mut inconsistent = original;
    inconsistent["items"][0]["available"] = json!(false);
    assert!(parse(&inconsistent).is_err());
    assert!(Catalog::parse(&vec![b' '; 1024 * 1024 + 1], &identity()).is_err());
}
