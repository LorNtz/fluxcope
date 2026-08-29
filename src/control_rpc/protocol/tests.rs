use super::{
    ControlError, ControlErrorCode, ControlOperation, ControlOperationKind, DeclaredClient,
    RPC_VERSION, RequestEnvelope, ResponseEnvelope, decode_request_payload,
    decode_response_payload,
};
use crate::control_rpc::test_support::{
    ENDPOINT, OTHER_ENDPOINT, OTHER_RUN_ID, RUN_ID, describe_result, instance_scope, request_value,
    run_id, scope_value, success_response_value,
};
use serde_json::{json, value::RawValue};
use std::time::{Duration, Instant};

fn request_with(mut change: impl FnMut(&mut serde_json::Value)) -> Vec<u8> {
    let mut value = request_value(RUN_ID, 1_000);
    change(&mut value);
    serde_json::to_vec(&value).expect("request JSON")
}

fn assert_invalid_request(payload: &[u8]) {
    let error = decode_request_payload(payload, Instant::now()).expect_err("invalid request");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[test]
fn request_envelope_serializes_operation_and_arguments_at_top_level() {
    let envelope = RequestEnvelope {
        protocol_version: RPC_VERSION,
        request_id: "request-1".to_owned(),
        run_id: run_id(),
        deadline_ms: 1_000,
        client: DeclaredClient {
            name: "test-client".to_owned(),
            version: "1.0".to_owned(),
        },
        operation: ControlOperationKind::DescribeInstance,
        arguments: RawValue::from_string("{}".to_owned()).expect("raw arguments"),
    };

    let actual = serde_json::to_value(envelope).expect("serialize request envelope");

    assert_eq!(
        actual,
        json!({
            "protocol_version": 1,
            "request_id": "request-1",
            "run_id": RUN_ID,
            "deadline_ms": 1_000,
            "client": {"name": "test-client", "version": "1.0"},
            "operation": "describe_instance",
            "arguments": {}
        })
    );
}

#[test]
fn response_envelopes_serialize_exactly_one_typed_outcome() {
    let success = ResponseEnvelope::success("request-1".to_owned(), describe_result());
    let failure = ResponseEnvelope::error(
        "request-2".to_owned(),
        ControlError {
            code: ControlErrorCode::InstanceUnavailable,
            message: "instance unavailable".to_owned(),
            retryable: true,
            details: json!({"reason": "closed"}),
            local_transport_cause: None,
        },
    );

    assert_eq!(
        serde_json::to_value(success).expect("serialize success"),
        success_response_value("request-1", scope_value(ENDPOINT, RUN_ID))
    );
    assert_eq!(
        serde_json::to_value(failure).expect("serialize failure"),
        json!({
            "protocol_version": 1,
            "request_id": "request-2",
            "error": {
                "code": "instance_unavailable",
                "message": "instance unavailable",
                "retryable": true,
                "details": {"reason": "closed"}
            }
        })
    );
}

#[test]
fn valid_request_becomes_a_typed_operation() {
    let received_at = Instant::now();
    let request = decode_request_payload(
        &serde_json::to_vec(&request_value(RUN_ID, 250)).expect("request JSON"),
        received_at,
    )
    .expect("valid request");

    assert_eq!(request.request_id, "request-1");
    assert_eq!(request.run_id, run_id());
    assert_eq!(request.client.name, "test-client");
    assert_eq!(request.client.version, "1.0");
    assert_eq!(request.operation, ControlOperation::DescribeInstance);
    assert_eq!(request.deadline, received_at + Duration::from_millis(250));
}

#[test]
fn request_deadline_is_clamped_to_the_operation_maximum() {
    let received_at = Instant::now();
    let request = decode_request_payload(
        &serde_json::to_vec(&request_value(RUN_ID, u64::MAX)).expect("request JSON"),
        received_at,
    )
    .expect("valid request");

    assert_eq!(request.deadline, received_at + Duration::from_secs(30));
}

#[test]
fn request_rejects_unknown_and_duplicate_envelope_fields() {
    let unknown = br#"{"protocol_version":1,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0"},"operation":"describe_instance","arguments":{},"extra":true}"#;
    let duplicate = br#"{"protocol_version":1,"protocol_version":1,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0"},"operation":"describe_instance","arguments":{}}"#;

    assert_invalid_request(unknown);
    assert_invalid_request(duplicate);
}

#[test]
fn request_rejects_unknown_and_duplicate_client_fields() {
    let unknown = br#"{"protocol_version":1,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0","extra":true},"operation":"describe_instance","arguments":{}}"#;
    let duplicate = br#"{"protocol_version":1,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","name":"other","version":"1.0"},"operation":"describe_instance","arguments":{}}"#;

    assert_invalid_request(unknown);
    assert_invalid_request(duplicate);
}

#[test]
fn describe_instance_rejects_unknown_and_duplicate_argument_fields() {
    let unknown = request_with(|value| value["arguments"] = json!({"extra": true}));
    let duplicate = br#"{"protocol_version":1,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0"},"operation":"describe_instance","arguments":{"extra":1,"extra":2}}"#;

    assert_invalid_request(&unknown);
    assert_invalid_request(duplicate);
}

#[test]
fn request_rejects_trailing_json_data() {
    let mut payload = serde_json::to_vec(&request_value(RUN_ID, 1_000)).expect("request JSON");
    payload.extend_from_slice(br#"{}"#);

    assert_invalid_request(&payload);
}

#[test]
fn request_rejects_unknown_operation_and_protocol_version() {
    let unknown_operation = request_with(|value| value["operation"] = json!("not_an_operation"));
    let wrong_version = request_with(|value| value["protocol_version"] = json!(2));

    assert_invalid_request(&unknown_operation);
    let error = decode_request_payload(&wrong_version, Instant::now()).expect_err("wrong version");
    assert_eq!(error.code, ControlErrorCode::RpcVersionMismatch);
}

#[test]
fn request_rejects_noncanonical_run_id() {
    let payload = request_with(|value| value["run_id"] = json!("not-a-run-id"));

    assert_invalid_request(&payload);
}

#[test]
fn request_rejects_empty_or_overlong_identifiers_by_utf8_bytes() {
    let empty_request_id = request_with(|value| value["request_id"] = json!(""));
    let long_request_id = request_with(|value| value["request_id"] = json!("é".repeat(65)));
    let empty_client_name = request_with(|value| value["client"]["name"] = json!(""));
    let long_client_name = request_with(|value| value["client"]["name"] = json!("é".repeat(65)));
    let empty_client_version = request_with(|value| value["client"]["version"] = json!(""));
    let long_client_version =
        request_with(|value| value["client"]["version"] = json!("é".repeat(65)));

    for payload in [
        empty_request_id,
        long_request_id,
        empty_client_name,
        long_client_name,
        empty_client_version,
        long_client_version,
    ] {
        assert_invalid_request(&payload);
    }
}

#[test]
fn response_rejects_both_or_neither_outcome() {
    let both = json!({
        "protocol_version": 1,
        "request_id": "request-1",
        "result": {
            "operation": "describe_instance",
            "instance": scope_value(ENDPOINT, RUN_ID)
        },
        "error": {
            "code": "instance_unavailable",
            "message": "closed",
            "retryable": true,
            "details": {}
        }
    });
    let neither = json!({"protocol_version": 1, "request_id": "request-1"});

    for value in [both, neither] {
        let payload = serde_json::to_vec(&value).expect("response JSON");
        let error = decode_response_payload(&payload, "request-1", &instance_scope())
            .expect_err("invalid outcome cardinality");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn response_rejects_duplicate_fields_and_trailing_json() {
    let duplicate = br#"{"protocol_version":1,"request_id":"request-1","result":{"operation":"describe_instance","instance":{"proxy_endpoint":"127.0.0.1:19001","run_id":"AAAAAAAAAAAAAAAAAAAAAA"}},"result":{"operation":"describe_instance","instance":{"proxy_endpoint":"127.0.0.1:19001","run_id":"AAAAAAAAAAAAAAAAAAAAAA"}}}"#;
    let mut trailing = serde_json::to_vec(&success_response_value(
        "request-1",
        scope_value(ENDPOINT, RUN_ID),
    ))
    .expect("response JSON");
    trailing.extend_from_slice(br#"{}"#);

    for payload in [duplicate.as_slice(), trailing.as_slice()] {
        let error = decode_response_payload(payload, "request-1", &instance_scope())
            .expect_err("strict response envelope");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn response_rejects_unknown_fields_versions_and_error_codes() {
    let unknown_field = json!({
        "protocol_version": 1,
        "request_id": "request-1",
        "result": {
            "operation": "describe_instance",
            "instance": scope_value(ENDPOINT, RUN_ID)
        },
        "extra": true
    });
    let mut wrong_version = serde_json::to_value(ResponseEnvelope::success(
        "request-1".to_owned(),
        describe_result(),
    ))
    .expect("complete describe response");
    wrong_version["protocol_version"] = json!(2);
    let unknown_error_code = json!({
        "protocol_version": 1,
        "request_id": "request-1",
        "error": {
            "code": "made_up",
            "message": "bad",
            "retryable": false,
            "details": {}
        }
    });

    let error = decode_response_payload(
        &serde_json::to_vec(&unknown_field).expect("response JSON"),
        "request-1",
        &instance_scope(),
    )
    .expect_err("unknown response field");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    let error = decode_response_payload(
        &serde_json::to_vec(&wrong_version).expect("response JSON"),
        "request-1",
        &instance_scope(),
    )
    .expect_err("wrong response version");
    assert_eq!(error.code, ControlErrorCode::RpcVersionMismatch);

    let error = decode_response_payload(
        &serde_json::to_vec(&unknown_error_code).expect("response JSON"),
        "request-1",
        &instance_scope(),
    )
    .expect_err("unknown error code");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[test]
fn response_rejects_mismatched_request_id_and_instance_scope() {
    let matching = success_response_value("different-request", scope_value(ENDPOINT, RUN_ID));
    let error = decode_response_payload(
        &serde_json::to_vec(&matching).expect("response JSON"),
        "request-1",
        &instance_scope(),
    )
    .expect_err("mismatched request ID");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    for scope in [
        scope_value(OTHER_ENDPOINT, RUN_ID),
        scope_value(ENDPOINT, OTHER_RUN_ID),
    ] {
        let response = success_response_value("request-1", scope);
        let error = decode_response_payload(
            &serde_json::to_vec(&response).expect("response JSON"),
            "request-1",
            &instance_scope(),
        )
        .expect_err("mismatched instance scope");
        assert_eq!(error.code, ControlErrorCode::InstanceGenerationConflict);
    }
}
