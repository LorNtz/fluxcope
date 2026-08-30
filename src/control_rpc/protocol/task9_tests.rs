use super::{
    ControlErrorCode, ControlOperation, ControlOperationKind, ControlResult, DeclaredClient,
    RPC_VERSION, RequestEnvelope, ResponseEnvelope, decode_request_payload,
    decode_response_payload,
};
use crate::{
    capture::{CaptureRecord, CaptureSequence, CaptureSnapshotMode, CapturedExchange},
    control::{
        CaptureMilestone, WaitForCaptureRequest, WaitForCaptureResult,
        capture_query::{CaptureQuery, CompactCapture},
    },
    control_rpc::test_support::{ENDPOINT, RUN_ID, instance_scope, run_id, scope_value},
};
use hyper::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn client() -> DeclaredClient {
    DeclaredClient {
        name: "task-9-protocol-test".to_owned(),
        version: "1".to_owned(),
    }
}

fn wait_operation(timeout_ms: Option<u64>) -> ControlOperation {
    ControlOperation::WaitForCapture(Box::new(WaitForCaptureRequest {
        query: CaptureQuery {
            method: Some("POST".to_owned()),
            ..CaptureQuery::default()
        },
        milestone: CaptureMilestone::ResponseStarted,
        timeout_ms,
    }))
}

fn wait_request_value(deadline_ms: u64) -> Value {
    serde_json::to_value(
        RequestEnvelope::new(
            "task-9-request".to_owned(),
            run_id(),
            deadline_ms,
            client(),
            wait_operation(Some(45_000)),
        )
        .expect("wait request envelope"),
    )
    .expect("wait request JSON")
}

fn compact_capture(sequence: u64) -> CompactCapture {
    let record = CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: Method::POST,
        uri: "https://example.test/wait".to_owned(),
        mapped_uri: None,
        local_path: None,
        status: Some(202),
        req_headers: vec![],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    });
    CompactCapture::from_snapshot(&record.snapshot(CaptureSnapshotMode::MetadataOnly))
}

#[test]
fn wait_for_capture_private_request_round_trips_strict_snake_case_arguments() {
    let value = wait_request_value(45_000);
    assert_eq!(value["operation"], json!("wait_for_capture"));
    assert_eq!(value["arguments"]["query"]["method"], json!("POST"));
    assert_eq!(value["arguments"]["milestone"], json!("response_started"));
    assert_eq!(value["arguments"]["timeout_ms"], json!(45_000));

    let received_at = Instant::now();
    let request = decode_request_payload(
        &serde_json::to_vec(&value).expect("request bytes"),
        received_at,
    )
    .expect("valid wait request");
    assert_eq!(request.operation, wait_operation(Some(45_000)));
    assert_eq!(request.deadline, received_at + Duration::from_secs(45));
}

#[test]
fn wait_for_capture_private_arguments_reject_unknown_fields_and_invalid_milestones() {
    let invalid_arguments = [
        json!({
            "query": {},
            "milestone": "request_seen",
            "timeout_ms": 1,
            "unexpected": true
        }),
        json!({"query": {}, "milestone": "response-started"}),
        json!({"query": {}, "milestone": "terminal"}),
        json!({"query": {}, "timeout_ms": 1}),
        json!({"query": {"unknown_filter": true}, "milestone": "request_seen"}),
    ];

    for arguments in invalid_arguments {
        let mut request = wait_request_value(1_000);
        request["arguments"] = arguments;
        let error = decode_request_payload(
            &serde_json::to_vec(&request).expect("request bytes"),
            Instant::now(),
        )
        .expect_err("strict wait arguments");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn wait_for_capture_deadline_cap_is_330_seconds_while_ordinary_calls_stay_at_30() {
    let received_at = Instant::now();
    let wait = decode_request_payload(
        &serde_json::to_vec(&wait_request_value(u64::MAX)).expect("wait request bytes"),
        received_at,
    )
    .expect("wait request");
    assert_eq!(wait.deadline, received_at + Duration::from_secs(330));

    let ordinary = RequestEnvelope::new(
        "ordinary-request".to_owned(),
        run_id(),
        u64::MAX,
        client(),
        ControlOperation::GetStatus,
    )
    .expect("ordinary envelope");
    let ordinary = decode_request_payload(
        &serde_json::to_vec(&ordinary).expect("ordinary request bytes"),
        received_at,
    )
    .expect("ordinary request");
    assert_eq!(ordinary.deadline, received_at + Duration::from_secs(30));
}

#[test]
fn wait_for_capture_result_round_trips_compact_metadata_and_repeats_identity() {
    let capture = compact_capture(7);
    let result = ControlResult::WaitForCapture {
        instance: instance_scope(),
        result: WaitForCaptureResult {
            matched: true,
            capture: Some(capture.clone()),
        },
    };
    assert_eq!(result.instance_scope(), &instance_scope());
    let envelope = ResponseEnvelope::success("task-9-request".to_owned(), result);
    let value = serde_json::to_value(&envelope).expect("wait response JSON");
    assert_eq!(
        value["result"],
        json!({
            "operation": "wait_for_capture",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "matched": true,
            "capture": capture
        })
    );

    let decoded = decode_response_payload(
        &serde_json::to_vec(&value).expect("wait response bytes"),
        "task-9-request",
        &instance_scope(),
        ControlOperationKind::WaitForCapture,
    )
    .expect("matching wait result");
    assert_eq!(decoded, envelope.result.expect("response result"));
}

#[test]
fn wait_for_capture_protocol_rejects_wrong_result_kind_and_inconsistent_match_shape() {
    let wait = ResponseEnvelope::success(
        "task-9-request".to_owned(),
        ControlResult::WaitForCapture {
            instance: instance_scope(),
            result: WaitForCaptureResult {
                matched: false,
                capture: None,
            },
        },
    );
    let bytes = serde_json::to_vec(&wait).expect("wait response bytes");
    let error = decode_response_payload(
        &bytes,
        "task-9-request",
        &instance_scope(),
        ControlOperationKind::SearchCaptures,
    )
    .expect_err("wrong operation/result identity");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    for (matched, capture) in [(true, Value::Null), (false, json!(compact_capture(8)))] {
        let malformed = json!({
            "protocol_version": RPC_VERSION,
            "request_id": "task-9-request",
            "result": {
                "operation": "wait_for_capture",
                "instance": scope_value(ENDPOINT, RUN_ID),
                "matched": matched,
                "capture": capture
            }
        });
        let error = decode_response_payload(
            &serde_json::to_vec(&malformed).expect("response bytes"),
            "task-9-request",
            &instance_scope(),
            ControlOperationKind::WaitForCapture,
        )
        .expect_err("matched and capture must agree");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn wait_for_capture_result_rejects_unknown_fields() {
    let value = json!({
        "protocol_version": RPC_VERSION,
        "request_id": "task-9-request",
        "result": {
            "operation": "wait_for_capture",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "matched": false,
            "capture": null,
            "headers": {"secret": "must never be in a compact wait result"}
        }
    });
    let error = decode_response_payload(
        &serde_json::to_vec(&value).expect("response bytes"),
        "task-9-request",
        &instance_scope(),
        ControlOperationKind::WaitForCapture,
    )
    .expect_err("unknown wait result field");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}
