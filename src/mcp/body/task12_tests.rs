use schemars::schema_for;
use serde_json::{Value, json};

use super::{ExtractCaptureBodyInput, SearchCaptureBodyInput};
use crate::{
    capture::BodySide, control::body::ExtractSelector, control_rpc::protocol::ControlOperation,
};

const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

fn required_input(operation: Value) -> Value {
    let mut value = json!({
        "instance": {"proxy_endpoint": "127.0.0.1:19012", "run_id": RUN_ID},
        "capture_id": 41,
        "capture_revision": 7,
        "side": "response"
    });
    value
        .as_object_mut()
        .expect("input object")
        .extend(operation.as_object().expect("operation object").clone());
    value
}

#[test]
fn task12_inputs_are_closed_and_build_exact_private_operations() {
    let search: SearchCaptureBodyInput = serde_json::from_value(required_input(json!({
        "query": "Straße",
        "limit": 50,
        "context_bytes": 1024
    })))
    .expect("search input");
    match search.operation().expect("search operation") {
        ControlOperation::SearchCaptureBody(request) => {
            assert_eq!(request.capture_id.value(), 41);
            assert_eq!(request.capture_revision, 7);
            assert_eq!(request.side, BodySide::Response);
            assert_eq!(request.query, "Straße");
            assert_eq!(request.limit, 50);
            assert_eq!(request.context_bytes, 1024);
        }
        other => panic!("unexpected operation: {other:?}"),
    }

    let extract: ExtractCaptureBodyInput = serde_json::from_value(required_input(json!({
        "selector": {"kind": "form_field", "key": "city"}
    })))
    .expect("extract input");
    match extract.operation().expect("extract operation") {
        ControlOperation::ExtractCaptureBody(request) => {
            assert_eq!(request.capture_id.value(), 41);
            assert_eq!(request.capture_revision, 7);
            assert_eq!(request.side, BodySide::Response);
            assert_eq!(
                request.selector,
                ExtractSelector::FormField {
                    key: "city".to_owned()
                }
            );
        }
        other => panic!("unexpected operation: {other:?}"),
    }

    let mut unknown = required_input(json!({"query": "x"}));
    unknown["unexpected"] = json!(true);
    assert!(serde_json::from_value::<SearchCaptureBodyInput>(unknown).is_err());
}

#[test]
fn task12_schemas_require_exact_target_and_bound_optional_search_controls() {
    let search = serde_json::to_value(schema_for!(SearchCaptureBodyInput)).expect("search schema");
    let extract =
        serde_json::to_value(schema_for!(ExtractCaptureBodyInput)).expect("extract schema");
    for (schema, operation_field) in [(&search, "query"), (&extract, "selector")] {
        assert_eq!(schema["additionalProperties"], json!(false));
        let required = schema["required"].as_array().expect("required fields");
        for field in [
            "instance",
            "capture_id",
            "capture_revision",
            "side",
            operation_field,
        ] {
            assert!(required.contains(&json!(field)), "missing {field}");
        }
    }
    assert_eq!(search["properties"]["limit"]["minimum"], json!(1));
    assert_eq!(search["properties"]["limit"]["maximum"], json!(50));
    assert_eq!(search["properties"]["context_bytes"]["minimum"], json!(0));
    assert_eq!(
        search["properties"]["context_bytes"]["maximum"],
        json!(1024)
    );
}
