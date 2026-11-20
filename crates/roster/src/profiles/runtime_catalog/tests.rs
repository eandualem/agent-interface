use super::*;
use serde_json::{Value, json};

fn identity() -> Identity {
    Identity {
        profile_id: "profile-a".into(),
        installation_id: "installation-a".into(),
        lifecycle_id: "lifecycle-a1".into(),
    }
}
#[test]
fn a_served_catalog_is_this_machines_list_for_the_profile_asked() {
    // As the Rust service's `backbone runtimes --json` prints it.
    let row = |id: &str, available: bool, setup: bool| {
        json!({"id": id, "display_name": id, "available": available,
            "setup_supported": setup, "reports_state": "terminal", "models": ["opus"],
            "efforts": ["opus:high"], "unattended": {"supported": false, "args": [], "sandboxed": null},
            "authentication": "unknown"})
    };
    let wire = json!({"schema_version": 1, "items": [
        row("claude", true, true), row("codex", false, true), row("antigravity", true, false)
    ]});
    let catalog = Catalog::served(&serde_json::to_vec(&wire).unwrap(), &identity()).unwrap();
    assert_eq!(catalog.identity(), &identity());
    let compatible: Vec<_> = catalog
        .items()
        .iter()
        .map(|r| r.available && r.setup_supported)
        .collect();
    assert_eq!(compatible, [true, false, false]);
    assert_eq!(catalog.items()[0].authentication, Authentication::Unknown);
    let mut duplicate = wire.clone();
    duplicate["items"]
        .as_array_mut()
        .unwrap()
        .push(row("claude", true, true));
    assert!(Catalog::served(&serde_json::to_vec(&duplicate).unwrap(), &identity()).is_err());
    let mut other = wire.clone();
    other["schema_version"] = json!(2);
    assert_eq!(
        Catalog::served(&serde_json::to_vec(&other).unwrap(), &identity()).unwrap_err(),
        Error::Unsupported
    );
}

#[test]
fn a_served_catalog_keeps_authentication_and_refuses_malformed_rows() {
    let row = json!({"id": "codex", "display_name": "Codex", "available": true,
        "setup_supported": true, "models": ["gpt"], "efforts": ["high"],
        "authentication": "authenticated"});
    let decode = |items: Value| {
        let wire = json!({"schema_version": 1, "items": items});
        Catalog::served(&serde_json::to_vec(&wire).unwrap(), &identity())
    };
    for (status, expected) in [
        ("authenticated", Authentication::Authenticated),
        ("not_authenticated", Authentication::NotAuthenticated),
        ("unavailable", Authentication::Unavailable),
        ("unsupported", Authentication::Unsupported),
        ("not_applicable", Authentication::NotApplicable),
        ("unknown", Authentication::Unknown),
        ("future_status", Authentication::Unknown),
    ] {
        let mut item = row.clone();
        item["authentication"] = json!(status);
        let catalog = decode(json!([item])).unwrap();
        assert_eq!(catalog.items()[0].authentication, expected, "{status}");
    }
    assert!(decode(json!([])).unwrap().items().is_empty());
    for (field, value) in [
        ("display_name", json!(" ")),
        ("display_name", json!("Co\ndex")),
        ("models", json!(["gpt", "gpt"])),
        ("efforts", json!(["high", "high"])),
    ] {
        let mut item = row.clone();
        item[field] = value.clone();
        assert!(decode(json!([item])).is_err(), "{field}: {value}");
    }
    // Only the service's object, within its size, is read.
    assert_eq!(
        Catalog::served(b"[1, []]", &identity()).unwrap_err(),
        Error::Invalid
    );
    assert_eq!(
        Catalog::served(&vec![b' '; 1024 * 1024 + 1], &identity()).unwrap_err(),
        Error::Invalid
    );
    // A newer schema is told apart from a malformed one, before its items.
    let newer = json!({"schema_version": 2, "items": "a shape to come"});
    assert_eq!(
        Catalog::served(&serde_json::to_vec(&newer).unwrap(), &identity()).unwrap_err(),
        Error::Unsupported
    );
}
