use schemars::schema_for;
use serde_json::{Value, json};

use super::*;
use crate::{
    control::json_walk::{FieldMatchMode, JsonPointerPattern},
    control_rpc::protocol::ControlOperation,
};

const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

fn find_input() -> FindJsonPointersInput {
    serde_json::from_value(json!({
        "instance": {"proxy_endpoint": "127.0.0.1:19011", "run_id": RUN_ID},
        "capture_id": 41,
        "capture_revision": 7,
        "side": "response",
        "field_name": "userId",
        "match_mode": "unicode_casefold_exact",
        "limit": 20
    }))
    .expect("find input")
}

fn probe_input() -> ProbeJsonPointerPatternInput {
    serde_json::from_value(json!({
        "instance": {"proxy_endpoint": "127.0.0.1:19011", "run_id": RUN_ID},
        "capture_id": 41,
        "capture_revision": 7,
        "side": "request",
        "pattern": "/orders/*/userId"
    }))
    .expect("probe input")
}

#[test]
fn json_tool_schemas_require_revision_pinned_target_and_reject_unknown_fields() {
    for schema in [
        serde_json::to_value(schema_for!(FindJsonPointersInput)).expect("find schema"),
        serde_json::to_value(schema_for!(ProbeJsonPointerPatternInput)).expect("probe schema"),
    ] {
        let text = schema.to_string();
        for required in [
            "proxy_endpoint",
            "run_id",
            "capture_id",
            "capture_revision",
            "side",
        ] {
            assert!(text.contains(required), "missing {required}: {text}");
        }
        assert!(text.contains("additionalProperties"));
    }
    let mut value = serde_json::to_value(find_input()).expect("serialize");
    value
        .as_object_mut()
        .expect("object")
        .insert("unknown".to_owned(), Value::Bool(true));
    assert!(serde_json::from_value::<FindJsonPointersInput>(value).is_err());
}

#[test]
fn public_inputs_create_exact_typed_rpc_operations() {
    let find = find_input();
    assert_eq!(find.match_mode, FieldMatchMode::UnicodeCasefoldExact);
    match find.operation().expect("operation") {
        ControlOperation::FindJsonPointers(request) => {
            assert_eq!(request.capture_id.value(), 41);
            assert_eq!(request.capture_revision, 7);
            assert_eq!(request.side, BodySide::Response);
            assert_eq!(request.field_name, "userId");
            assert_eq!(request.limit, 20);
        }
        other => panic!("unexpected operation: {other:?}"),
    }

    let probe = probe_input();
    assert_eq!(
        probe.pattern,
        JsonPointerPattern::parse("/orders/*/userId").expect("pattern")
    );
    match probe.operation().expect("operation") {
        ControlOperation::ProbeJsonPointerPattern(request) => {
            assert_eq!(request.capture_id.value(), 41);
            assert_eq!(request.capture_revision, 7);
            assert_eq!(request.side, BodySide::Request);
            assert_eq!(request.pattern.as_str(), "/orders/*/userId");
        }
        other => panic!("unexpected operation: {other:?}"),
    }
}

#[test]
fn invalid_pointer_patterns_and_result_limits_fail_before_dispatch() {
    let mut recursive = serde_json::to_value(probe_input()).expect("serialize");
    recursive["pattern"] = json!("/**");
    assert!(serde_json::from_value::<ProbeJsonPointerPatternInput>(recursive).is_err());

    let mut excessive = serde_json::to_value(find_input()).expect("serialize");
    excessive["limit"] = json!(MAX_JSON_EXAMPLES + 1);
    let input: FindJsonPointersInput = serde_json::from_value(excessive).expect("typed input");
    assert!(input.operation().is_err());
}

#[test]
fn public_results_never_gain_a_value_field_or_serialize_secrets() {
    let output = FindJsonPointersOutput {
        instance: crate::mcp::schema::InstanceSelector {
            proxy_endpoint: Some("127.0.0.1:19011".parse().expect("endpoint")),
            run_id: Some(RUN_ID.parse().expect("run")),
        },
        result: crate::control::json_walk::FindJsonPointersResult {
            status: crate::control::json_walk::JsonInspectionStatus::Matched,
            capture_revision: 7,
            matches: vec![crate::control::json_walk::JsonFieldMatch {
                pointer: crate::control::json_walk::JsonPointer::parse("/password")
                    .expect("pointer"),
                value_type: crate::control::json_walk::JsonType::String,
                object_child_count: None,
                array_length: None,
                scalar_encoded_bytes: Some(31),
            }],
            total_matches: 1,
            omitted_matches: 0,
            inspected_bytes: 42,
            source_truncated: false,
            truncated: false,
        },
    };
    let serialized = serde_json::to_string(&output).expect("serialize");
    assert!(!serialized.contains("TASK11-SECRET"));
    assert!(!serialized.contains("\"value\""));
}
