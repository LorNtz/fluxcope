use super::*;
use serde_json::json;

fn input(operation: serde_json::Value) -> serde_json::Value {
    json!({
        "instance": {"proxy_endpoint": "127.0.0.1:8800", "run_id": "AAAAAAAAAAAAAAAAAAAAAA"},
        "expected_settings_revision": 7,
        "operation": operation,
        "urls": ["https://api.example/a"],
    })
}

#[test]
fn preview_rejects_ambiguous_operations_incomplete_identity_and_unbounded_urls() {
    let valid = input(
        json!({"kind": "set_mapping_rule_enabled", "preset": "dev", "table": "local", "index": 1, "enabled": true}),
    );
    serde_json::from_value::<PreviewMappingMutationInput>(valid.clone()).unwrap();
    for operation in [
        json!({"kind": "set_mapping_rule_enabled", "preset": "dev", "table": "local", "index": 1, "enable": true}),
        json!({"kind": "set_active_preset"}),
        json!({"kind": "create_mapping_rule", "preset": "dev", "table": "local", "rule": {"from": "https://api.example/a", "to": "/tmp/mock", "enable": true}}),
        json!({"kind": "replace_proxy", "proxy": {}}),
    ] {
        assert!(serde_json::from_value::<PreviewMappingMutationInput>(input(operation)).is_err());
    }
    for field in ["proxy_endpoint", "run_id"] {
        let mut value = valid.clone();
        value["instance"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<PreviewMappingMutationInput>(value).is_err());
    }
    let mut missing_revision = valid.clone();
    missing_revision
        .as_object_mut()
        .unwrap()
        .remove("expected_settings_revision");
    assert!(serde_json::from_value::<PreviewMappingMutationInput>(missing_revision).is_err());
    let mut over_limit = valid;
    over_limit["urls"] = json!(vec!["https://api.example/a"; 17]);
    assert!(
        serde_json::from_value::<PreviewMappingMutationInput>(over_limit)
            .unwrap()
            .into_operation()
            .is_err()
    );
}
