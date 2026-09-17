use super::{
    ControlError, ControlErrorCode, ControlOperation, ControlOperationKind, ControlResult,
    DeclaredClient, RPC_VERSION, RequestEnvelope, ResponseEnvelope, decode_request_payload,
    decode_response_payload,
};
use crate::{
    capture::CaptureSequence,
    control::capture_query::{CaptureQuery, CaptureSearchCursor},
    control_rpc::test_support::{
        ENDPOINT, OTHER_ENDPOINT, OTHER_RUN_ID, RUN_ID, describe_result, instance_scope,
        request_value, run_id, scope_value, success_response_value,
    },
    settings::{ConfigMode, PersistenceMode},
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
            "protocol_version": RPC_VERSION,
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
            "protocol_version": RPC_VERSION,
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
    let unknown = br#"{"protocol_version":2,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0"},"operation":"describe_instance","arguments":{},"extra":true}"#;
    let duplicate = br#"{"protocol_version":2,"protocol_version":2,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0"},"operation":"describe_instance","arguments":{}}"#;

    assert_invalid_request(unknown);
    assert_invalid_request(duplicate);
}

#[test]
fn request_rejects_unknown_and_duplicate_client_fields() {
    let unknown = br#"{"protocol_version":2,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0","extra":true},"operation":"describe_instance","arguments":{}}"#;
    let duplicate = br#"{"protocol_version":2,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","name":"other","version":"1.0"},"operation":"describe_instance","arguments":{}}"#;

    assert_invalid_request(unknown);
    assert_invalid_request(duplicate);
}

#[test]
fn describe_instance_rejects_unknown_and_duplicate_argument_fields() {
    let unknown = request_with(|value| value["arguments"] = json!({"extra": true}));
    let duplicate = br#"{"protocol_version":2,"request_id":"request-1","run_id":"AAAAAAAAAAAAAAAAAAAAAA","deadline_ms":1000,"client":{"name":"test-client","version":"1.0"},"operation":"describe_instance","arguments":{"extra":1,"extra":2}}"#;

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
    let wrong_version = request_with(|value| value["protocol_version"] = json!(1));

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
        "protocol_version": RPC_VERSION,
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
    let neither = json!({"protocol_version": RPC_VERSION, "request_id": "request-1"});

    for value in [both, neither] {
        let payload = serde_json::to_vec(&value).expect("response JSON");
        let error = decode_response_payload(
            &payload,
            "request-1",
            &instance_scope(),
            ControlOperationKind::DescribeInstance,
        )
        .expect_err("invalid outcome cardinality");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn response_rejects_duplicate_fields_and_trailing_json() {
    let duplicate = br#"{"protocol_version":2,"request_id":"request-1","result":{"operation":"describe_instance","instance":{"proxy_endpoint":"127.0.0.1:19001","run_id":"AAAAAAAAAAAAAAAAAAAAAA"}},"result":{"operation":"describe_instance","instance":{"proxy_endpoint":"127.0.0.1:19001","run_id":"AAAAAAAAAAAAAAAAAAAAAA"}}}"#;
    let mut trailing = serde_json::to_vec(&success_response_value(
        "request-1",
        scope_value(ENDPOINT, RUN_ID),
    ))
    .expect("response JSON");
    trailing.extend_from_slice(br#"{}"#);

    for payload in [duplicate.as_slice(), trailing.as_slice()] {
        let error = decode_response_payload(
            payload,
            "request-1",
            &instance_scope(),
            ControlOperationKind::DescribeInstance,
        )
        .expect_err("strict response envelope");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn response_rejects_unknown_fields_versions_and_error_codes() {
    let unknown_field = json!({
        "protocol_version": RPC_VERSION,
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
    wrong_version["protocol_version"] = json!(1);
    let unknown_error_code = json!({
        "protocol_version": RPC_VERSION,
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
        ControlOperationKind::DescribeInstance,
    )
    .expect_err("unknown response field");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    let error = decode_response_payload(
        &serde_json::to_vec(&wrong_version).expect("response JSON"),
        "request-1",
        &instance_scope(),
        ControlOperationKind::DescribeInstance,
    )
    .expect_err("wrong response version");
    assert_eq!(error.code, ControlErrorCode::RpcVersionMismatch);

    let error = decode_response_payload(
        &serde_json::to_vec(&unknown_error_code).expect("response JSON"),
        "request-1",
        &instance_scope(),
        ControlOperationKind::DescribeInstance,
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
        ControlOperationKind::DescribeInstance,
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
            ControlOperationKind::DescribeInstance,
        )
        .expect_err("mismatched instance scope");
        assert_eq!(error.code, ControlErrorCode::InstanceGenerationConflict);
    }
}

fn task8_request(operation: &str, arguments: serde_json::Value, deadline_ms: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "protocol_version": RPC_VERSION,
        "request_id": "task-8-request",
        "run_id": RUN_ID,
        "deadline_ms": deadline_ms,
        "client": {"name": "test-client", "version": "1.0"},
        "operation": operation,
        "arguments": arguments
    }))
    .expect("request JSON")
}

#[test]
fn task8_requests_decode_to_operation_specific_typed_arguments() {
    let received_at = Instant::now();
    let cases = [
        ("get_status", json!({}), ControlOperation::GetStatus),
        (
            "set_recording_enabled",
            json!({"enabled": true}),
            ControlOperation::SetRecordingEnabled { enabled: true },
        ),
        (
            "search_captures",
            json!({
                "query": {"method": "get", "mapping_path": "remote_only"},
                "cursor": {"next_older_sequence": 30},
                "limit": 10
            }),
            ControlOperation::SearchCaptures {
                query: Box::new(CaptureQuery {
                    method: Some("get".to_owned()),
                    mapping_path: Some(crate::control::capture_query::MappingPath::RemoteOnly),
                    ..CaptureQuery::default()
                }),
                cursor: Some(CaptureSearchCursor::new(CaptureSequence::new(30))),
                limit: Some(10),
            },
        ),
        (
            "get_capture",
            json!({"capture_id": 7, "expected_revision": 3}),
            ControlOperation::GetCapture {
                capture_id: CaptureSequence::new(7),
                expected_revision: Some(3),
            },
        ),
    ];

    for (operation, arguments, expected) in cases {
        let request =
            decode_request_payload(&task8_request(operation, arguments, 5_000), received_at)
                .expect("valid Task 8 request");
        assert_eq!(request.operation, expected);
        assert_eq!(request.deadline, received_at + Duration::from_secs(5));
    }
}

#[test]
fn task8_operations_reject_unknown_missing_and_structurally_malformed_arguments() {
    let invalid = [
        ("get_status", json!({"unexpected": true})),
        ("set_recording_enabled", json!({})),
        (
            "set_recording_enabled",
            json!({"enabled": true, "unexpected": true}),
        ),
        (
            "search_captures",
            json!({"query": {}, "cursor": "30", "limit": 10}),
        ),
        (
            "search_captures",
            json!({"query": {"mapping_path": "remote-local"}}),
        ),
        ("get_capture", json!({})),
        ("get_capture", json!({"capture_id": "seven"})),
        ("get_capture", json!({"capture_id": 7, "unexpected": true})),
    ];

    for (operation, arguments) in invalid {
        let error =
            decode_request_payload(&task8_request(operation, arguments, 1_000), Instant::now())
                .expect_err("invalid operation arguments");
        assert_eq!(
            error.code,
            ControlErrorCode::InvalidArgument,
            "{operation} should reject malformed arguments"
        );
    }

    let mut trailing = task8_request("get_status", json!({}), 1_000);
    trailing.extend_from_slice(br#" true"#);
    assert_invalid_request(&trailing);
}

#[test]
fn all_ordinary_task8_deadlines_are_clamped_to_thirty_seconds() {
    let received_at = Instant::now();
    for (operation, arguments) in [
        ("get_status", json!({})),
        ("set_recording_enabled", json!({"enabled": false})),
        ("search_captures", json!({"query": {}})),
        ("get_capture", json!({"capture_id": 1})),
    ] {
        let request =
            decode_request_payload(&task8_request(operation, arguments, u64::MAX), received_at)
                .expect("valid request");
        assert_eq!(
            request.deadline,
            received_at + Duration::from_secs(30),
            "{operation} deadline"
        );
    }
}

#[test]
fn task8_results_have_exact_tagged_shapes_and_repeat_instance_identity() {
    let status = ControlResult::GetStatus {
        local_proxy_url: format!("http://{ENDPOINT}"),
        fluxcope_version: "0.1.0-test".to_owned(),
        rpc_version: RPC_VERSION,
        config_source: None,
        instance: instance_scope(),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        recording_enabled: true,
        retained_capture_count: 4,
        settings_revision: 9,
        mapping: Default::default(),
        capture_store: Default::default(),
        capture_change_epoch: 0,
        metrics: Default::default(),
        private_rpc: Default::default(),
        body_work: Default::default(),
        search_work: Default::default(),
        audit: Default::default(),
    };
    let recording = ControlResult::SetRecordingEnabled {
        instance: instance_scope(),
        previous: false,
        current: true,
    };
    let search = ControlResult::SearchCaptures {
        instance: instance_scope(),
        captures: Vec::new(),
        next_cursor: None,
    };

    assert_eq!(
        serde_json::to_value(status).expect("status JSON"),
        json!({
            "operation": "get_status",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "local_proxy_url": format!("http://{ENDPOINT}"),
            "fluxcope_version": "0.1.0-test",
            "rpc_version": RPC_VERSION,
            "config_source": null,
            "config_mode": "temporary",
            "persistence": "ephemeral",
            "recording_enabled": true,
            "retained_capture_count": 4,
            "mapping": {
                "configured": false,
                "enabled": false,
                "active_preset": null,
                "map_remote_enabled": null,
                "map_local_enabled": null
            },
            "capture_store": {
                "revision": 0,
                "retained_bytes": 0,
                "maximum_retained_bytes": 0,
                "maximum_retained_records": 0
            },
            "capture_change_epoch": 0,
            "metrics": {
                "capture": {
                    "exchanges_not_admitted": 0,
                    "memory_pressure": 0,
                    "previews_per_body_limited": 0,
                    "previews_memory_limited": 0,
                    "metadata_truncated": 0
                },
                "decode": {
                    "rejected": 0,
                    "superseded": 0,
                    "output_limited": 0,
                    "failed": 0
                },
                "logging": {
                    "producer_dropped": 0,
                    "tui_dropped": 0,
                    "records_truncated": 0
                }
            },
            "private_rpc": {
                "active": 0,
                "maximum_active": 0
            },
            "body_work": {
                "active": 0,
                "queued": 0,
                "queued_bytes": 0,
                "maximum_active": 0,
                "maximum_queued": 0,
                "maximum_queued_bytes": 0,
                "rejected": 0
            },
            "search_work": {
                "capture_searches_active": 0,
                "maximum_capture_searches": 0,
                "detail_materializations_active": 0,
                "maximum_detail_materializations": 0
            },
            "audit": {
                "reads": [],
                "mutations": [],
                "response_delivery_failures": 0
            },
            "settings_revision": 9
        })
    );
    assert_eq!(
        serde_json::to_value(recording).expect("recording JSON"),
        json!({
            "operation": "set_recording_enabled",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "previous": false,
            "current": true
        })
    );
    assert_eq!(
        serde_json::to_value(search).expect("search JSON"),
        json!({
            "operation": "search_captures",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "captures": [],
            "next_cursor": null
        })
    );
}

#[test]
fn capture_not_found_and_revision_conflict_are_distinct_typed_errors() {
    let not_found = ControlError::new(
        ControlErrorCode::CaptureNotFound,
        "capture is not retained",
        false,
        json!({"capture_id": 7}),
    );
    let conflict = ControlError::new(
        ControlErrorCode::CaptureRevisionConflict,
        "capture revision changed",
        false,
        json!({"capture_id": 7, "expected_revision": 2, "current_revision": 3}),
    );

    assert_eq!(not_found.code.as_str(), "capture_not_found");
    assert_eq!(conflict.code.as_str(), "capture_revision_conflict");
    assert_eq!(
        serde_json::to_value(not_found).expect("not found JSON")["details"],
        json!({"capture_id": 7})
    );
    assert_eq!(
        serde_json::to_value(conflict).expect("conflict JSON")["details"],
        json!({"capture_id": 7, "expected_revision": 2, "current_revision": 3})
    );
}
