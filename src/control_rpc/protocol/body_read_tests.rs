use super::{
    ControlErrorCode, ControlOperation, ControlOperationKind, ControlResult, DeclaredClient,
    RPC_VERSION, RequestEnvelope, ResponseEnvelope, decode_request_payload,
    decode_response_payload,
};
use crate::{
    capture::{BodySide, BodyStreamState, CaptureSequence},
    control::body::{
        BodyContentRequest, BodyPage, BodyPageSource, BodyRange, BodyRepresentation,
        MAX_BODY_PAGE_LENGTH,
    },
    control_rpc::{
        framing::RESPONSE_MAX_BYTES,
        test_support::{ENDPOINT, RUN_ID, instance_scope, run_id, scope_value},
    },
    instance::InstanceIdentity,
};
use hyper::body::Bytes;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn client() -> DeclaredClient {
    DeclaredClient {
        name: "task-10-protocol-test".to_owned(),
        version: "1".to_owned(),
    }
}

fn request() -> BodyContentRequest {
    BodyContentRequest {
        capture_id: CaptureSequence::new(0),
        capture_revision: 3,
        side: BodySide::Response,
        representation: BodyRepresentation::Decoded,
        offset: 8_192,
        length: 4_096,
    }
}

fn request_value() -> Value {
    serde_json::to_value(
        RequestEnvelope::new(
            "task-10-request".to_owned(),
            run_id(),
            60_000,
            client(),
            ControlOperation::ReadCaptureBody(Box::new(request())),
        )
        .expect("body request envelope"),
    )
    .expect("body request JSON")
}

fn page(content: Bytes) -> BodyPage {
    BodyPage {
        content,
        media_type: Some("text/plain; charset=utf-8".to_owned()),
        requested_range: BodyRange {
            offset: 8_192,
            length: 4_096,
        },
        actual_range: BodyRange {
            offset: 8_192,
            length: 4,
        },
        total_bytes: 8_196,
        next_offset: None,
        source: BodyPageSource {
            stream: BodyStreamState::Complete,
            observed_bytes: 8_196,
            retained_bytes: 2_048,
            truncated: true,
            truncation_reason: None,
            decoded_encoding_chain: vec!["gzip".to_owned()],
            decoded_output_limited: false,
        },
    }
}

#[test]
fn read_capture_body_request_has_strict_kind_fields_and_ordinary_deadline() {
    let value = request_value();
    assert_eq!(value["operation"], json!("read_capture_body"));
    assert_eq!(
        value["arguments"],
        json!({
            "capture_id": 0,
            "capture_revision": 3,
            "side": "response",
            "representation": "decoded",
            "offset": 8192,
            "length": 4096
        })
    );
    let received_at = Instant::now();
    let decoded = decode_request_payload(
        &serde_json::to_vec(&value).expect("body request bytes"),
        received_at,
    )
    .expect("body request decode");
    assert_eq!(
        decoded.operation,
        ControlOperation::ReadCaptureBody(Box::new(request()))
    );
    assert_eq!(
        decoded.operation.kind(),
        ControlOperationKind::ReadCaptureBody
    );
    assert_eq!(decoded.deadline, received_at + Duration::from_secs(30));
}

#[test]
fn read_capture_body_arguments_reject_missing_unknown_and_invalid_fields() {
    for field in [
        "capture_id",
        "capture_revision",
        "side",
        "representation",
        "offset",
        "length",
    ] {
        let mut value = request_value();
        value["arguments"]
            .as_object_mut()
            .expect("body arguments object")
            .remove(field);
        let error = decode_request_payload(
            &serde_json::to_vec(&value).expect("missing-field request bytes"),
            Instant::now(),
        )
        .unwrap_err();
        assert_eq!(error.code, ControlErrorCode::InvalidArgument, "{field}");
    }

    let invalid = [
        json!({
            "capture_revision": 3,
            "side": "response",
            "representation": "decoded",
            "offset": 0,
            "length": 8192
        }),
        json!({
            "capture_id": 0,
            "side": "response",
            "representation": "decoded",
            "offset": 0,
            "length": 8192
        }),
        json!({
            "capture_id": 0,
            "capture_revision": 3,
            "side": "response",
            "representation": "decoded",
            "offset": 0,
            "length": 8192,
            "unexpected": true
        }),
        json!({
            "capture_id": 0,
            "capture_revision": 3,
            "side": "responses",
            "representation": "decoded",
            "offset": 0,
            "length": 8192
        }),
        json!({
            "capture_id": 0,
            "capture_revision": 3,
            "side": "response",
            "representation": "display",
            "offset": 0,
            "length": 8192
        }),
    ];
    for arguments in invalid {
        let mut value = request_value();
        value["arguments"] = arguments;
        let error = decode_request_payload(
            &serde_json::to_vec(&value).expect("invalid request bytes"),
            Instant::now(),
        )
        .expect_err("strict body arguments");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }

    let mut unknown_operation = request_value();
    unknown_operation["operation"] = json!("read_body");
    let error = decode_request_payload(
        &serde_json::to_vec(&unknown_operation).unwrap(),
        Instant::now(),
    )
    .expect_err("unknown operation");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[test]
fn read_capture_body_result_round_trips_base64_bytes_and_exact_identity() {
    let result = ControlResult::ReadCaptureBody {
        instance: instance_scope(),
        page: Box::new(page(Bytes::from_static(&[0xff, 0x00, b'a', b'b']))),
    };
    assert_eq!(result.kind(), ControlOperationKind::ReadCaptureBody);
    let envelope = ResponseEnvelope::success("task-10-request".to_owned(), result);
    let value = serde_json::to_value(&envelope).expect("body result JSON");
    assert_eq!(value["result"]["operation"], json!("read_capture_body"));
    assert_eq!(value["result"]["instance"], scope_value(ENDPOINT, RUN_ID));
    assert_eq!(value["result"]["page"]["content"], json!("/wBhYg=="));

    let decoded = decode_response_payload(
        &serde_json::to_vec(&value).expect("body response bytes"),
        "task-10-request",
        &instance_scope(),
        ControlOperationKind::ReadCaptureBody,
    )
    .expect("body response decode");
    assert_eq!(decoded, envelope.result.expect("response result"));
}

#[test]
fn read_capture_body_response_validates_operation_and_instance_identity() {
    let envelope = ResponseEnvelope::success(
        "task-10-request".to_owned(),
        ControlResult::ReadCaptureBody {
            instance: instance_scope(),
            page: Box::new(page(Bytes::from_static(b"body"))),
        },
    );
    let bytes = serde_json::to_vec(&envelope).expect("body response bytes");
    let error = decode_response_payload(
        &bytes,
        "task-10-request",
        &instance_scope(),
        ControlOperationKind::GetCapture,
    )
    .expect_err("wrong result kind");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    let other_identity = InstanceIdentity::new("127.0.0.1:19999".parse().unwrap()).unwrap();
    let other_scope = super::InstanceScope {
        proxy_endpoint: other_identity.proxy_endpoint(),
        run_id: other_identity.run_id().clone(),
    };
    let error = decode_response_payload(
        &bytes,
        "task-10-request",
        &other_scope,
        ControlOperationKind::ReadCaptureBody,
    )
    .expect_err("wrong response instance");
    assert_eq!(error.code, ControlErrorCode::InstanceGenerationConflict);
}

#[test]
fn read_capture_body_result_rejects_unknown_fields() {
    let value = json!({
        "protocol_version": RPC_VERSION,
        "request_id": "task-10-request",
        "result": {
            "operation": "read_capture_body",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "page": serde_json::to_value(page(Bytes::from_static(b"body"))).unwrap(),
            "filesystem_path": "/private/tmp/must-not-leak"
        }
    });
    let error = decode_response_payload(
        &serde_json::to_vec(&value).unwrap(),
        "task-10-request",
        &instance_scope(),
        ControlOperationKind::ReadCaptureBody,
    )
    .expect_err("unknown result field");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[test]
fn maximum_body_page_private_response_stays_within_the_eight_mib_frame_bound() {
    let envelope = ResponseEnvelope::success(
        "task-10-request".to_owned(),
        ControlResult::ReadCaptureBody {
            instance: instance_scope(),
            page: Box::new(page(Bytes::from(vec![0xff; MAX_BODY_PAGE_LENGTH]))),
        },
    );
    let encoded = serde_json::to_vec(&envelope).expect("maximum body response JSON");
    assert!(encoded.len() < RESPONSE_MAX_BYTES);
    assert!(
        encoded.len() > MAX_BODY_PAGE_LENGTH,
        "private bytes are base64 encoded"
    );
}
