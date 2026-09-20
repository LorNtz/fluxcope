use std::{
    str::FromStr,
    time::{Duration, Instant},
};

use serde_json::json;

use super::{ControlOperation, DeclaredClient, RequestEnvelope, decode_request_payload};
use crate::{
    capture::{BodySide, CaptureSequence},
    control::{
        body::{
            ExtractCaptureBodyRequest, ExtractSelector, SearchCaptureBodyRequest,
            SelectionContentRequest,
        },
        json_walk::JsonPointer,
    },
    instance::RunId,
};

fn run_id() -> RunId {
    RunId::from_str("AAAAAAAAAAAAAAAAAAAAAA").expect("run ID")
}

fn client() -> DeclaredClient {
    DeclaredClient {
        name: "task12-test".to_owned(),
        version: "1".to_owned(),
    }
}

#[test]
fn private_operations_round_trip_with_strict_boxed_arguments() {
    let operations = [
        ControlOperation::SearchCaptureBody(Box::new(SearchCaptureBodyRequest {
            capture_id: CaptureSequence::new(9),
            capture_revision: 3,
            side: BodySide::Response,
            query: "Straße".to_owned(),
            limit: 10,
            context_bytes: 160,
        })),
        ControlOperation::ExtractCaptureBody(Box::new(ExtractCaptureBodyRequest {
            capture_id: CaptureSequence::new(9),
            capture_revision: 3,
            side: BodySide::Response,
            selector: ExtractSelector::JsonPointer {
                pointer: JsonPointer::parse("/value").expect("pointer"),
            },
        })),
        ControlOperation::ReadSelectedBody(Box::new(SelectionContentRequest {
            capture_id: CaptureSequence::new(9),
            capture_revision: 3,
            side: BodySide::Response,
            selector: ExtractSelector::FormField {
                key: "city".to_owned(),
            },
            offset: 0,
            length: 8192,
        })),
    ];
    let received_at = Instant::now();
    for (index, operation) in operations.into_iter().enumerate() {
        let expected = operation.clone();
        let envelope = RequestEnvelope::new(
            format!("task12-{index}"),
            run_id(),
            30_000,
            client(),
            operation,
        )
        .expect("request envelope");
        let encoded = serde_json::to_vec(&envelope).expect("request JSON");
        let decoded = decode_request_payload(&encoded, received_at).expect("strict request");
        assert_eq!(decoded.operation, expected);
        assert_eq!(decoded.deadline, received_at + Duration::from_secs(30));
    }
}

#[test]
fn private_arguments_reject_unknown_fields_and_invalid_limits() {
    let envelope = json!({
        "protocol_version": super::RPC_VERSION,
        "request_id": "task12-invalid",
        "run_id": "AAAAAAAAAAAAAAAAAAAAAA",
        "deadline_ms": 1000,
        "client": {"name": "test", "version": "1"},
        "operation": "search_capture_body",
        "arguments": {
            "capture_id": 9,
            "capture_revision": 3,
            "side": "response",
            "query": "x",
            "limit": 51,
            "context_bytes": 160,
            "unexpected": true
        }
    });
    let error = decode_request_payload(
        &serde_json::to_vec(&envelope).expect("invalid envelope JSON"),
        Instant::now(),
    )
    .expect_err("closed arguments");
    assert_eq!(error.code.as_str(), "invalid_argument");
}
