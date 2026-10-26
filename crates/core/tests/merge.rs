//! Differential cases generated from the pinned Python model in the fixture.

use agent_interface_core::merge::merge_shared_layout;
use serde_json::Value;

#[test]
fn shared_merges_match_the_pinned_python_model() {
    let fixtures: Value = serde_json::from_str(include_str!("merge_cases.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let actual = merge_shared_layout(&case["base"], &case["mine"], &case["latest"]);
        if case["conflict"].as_bool().unwrap() {
            assert!(actual.is_err(), "{}", case["name"]);
        } else {
            assert_eq!(actual.unwrap(), case["expected"], "{}", case["name"]);
        }
    }
}
