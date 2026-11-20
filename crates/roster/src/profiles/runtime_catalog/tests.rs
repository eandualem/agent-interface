use super::*;
use serde_json::json;

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
