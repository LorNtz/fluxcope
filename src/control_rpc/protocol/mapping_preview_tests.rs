use super::*;
use crate::control_rpc::test_support::{RUN_ID, run_id};
use serde_json::json;

fn request(arguments: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "protocol_version": RPC_VERSION, "request_id": "preview-test", "run_id": RUN_ID,
        "deadline_ms": 5000, "client": {"name": "preview-test", "version": "1"},
        "operation": "preview_mapping_mutation", "arguments": arguments,
    }))
    .unwrap()
}

#[test]
fn preview_round_trips_and_rejects_private_nested_unknowns_and_url_bounds() {
    let operation = ControlOperation::PreviewMappingMutation {
        expected_revision: SettingsRevision::new(7),
        mutation: Box::new(MappingMutation::AppendLocalRule {
            preset: "dev".into(),
            rule: ProxyMapLocalRule {
                from: "https://api.example/a".into(),
                to: "/missing/mock.json".into(),
                enable: false,
            },
        }),
        urls: vec!["https://api.example/a".into()],
    };
    let run = run_id();
    let client = DeclaredClient {
        name: "preview-test".into(),
        version: "1".into(),
    };
    let envelope = OutboundRequestEnvelope::new("preview-test", &run, 5000, &client, &operation);
    let encoded = serde_json::to_vec(&envelope).unwrap();
    assert_eq!(
        decode_request_payload(&encoded, Instant::now())
            .unwrap()
            .operation,
        operation
    );
    let base = json!({"expected_revision": 7, "mutation": {"kind": "append_local_rule", "preset": "dev", "rule": {"from": "https://api.example/a", "to": "/missing/mock.json", "enable": false}}, "urls": []});
    let mut unknown = base.clone();
    unknown["mutation"]["rule"]["enabled"] = json!(true);
    assert!(decode_request_payload(&request(unknown), Instant::now()).is_err());
    let mut missing_revision = base.clone();
    missing_revision
        .as_object_mut()
        .unwrap()
        .remove("expected_revision");
    assert!(decode_request_payload(&request(missing_revision), Instant::now()).is_err());
    for urls in [
        json!([""]),
        json!(vec!["https://api.example/a"; 17]),
        json!(["x".repeat(crate::control::settings::mapping::MAX_PREVIEW_URL_BYTES + 1)]),
    ] {
        let mut arguments = base.clone();
        arguments["urls"] = urls;
        assert_eq!(
            decode_request_payload(&request(arguments), Instant::now())
                .unwrap_err()
                .code(),
            ControlErrorCode::InvalidArgument
        );
    }
}
