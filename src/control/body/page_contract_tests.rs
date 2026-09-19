use super::{
    BodyContentRequest, BodyPage, BodyPageSource, BodyRange, BodyRepresentation,
    DEFAULT_BODY_PAGE_LENGTH, MAX_BODY_PAGE_LENGTH, MAX_DECODED_CONTENT_BYTES,
    MAX_TERMINAL_DECODED_CACHE_BYTES,
};
use crate::{
    capture::{BodyPreviewLimit, BodySide, BodyStreamState, CaptureSequence},
    control_rpc::protocol::ControlErrorCode,
};
use hyper::body::Bytes;
use serde_json::json;

#[test]
fn body_request_and_page_wire_shape_is_binary_safe_and_exact() {
    let request = BodyContentRequest {
        capture_id: CaptureSequence::new(0),
        capture_revision: 9,
        side: BodySide::Response,
        representation: BodyRepresentation::Decoded,
        offset: 8_192,
        length: 4_096,
    };
    assert_eq!(
        serde_json::to_value(&request).expect("body request JSON"),
        json!({
            "capture_id": 0,
            "capture_revision": 9,
            "side": "response",
            "representation": "decoded",
            "offset": 8192,
            "length": 4096
        })
    );
    assert_eq!(
        serde_json::from_value::<BodyContentRequest>(json!({
            "capture_id": 0,
            "capture_revision": 9,
            "side": "response",
            "representation": "decoded",
            "offset": 8192,
            "length": 4096
        }))
        .expect("body request round trip"),
        request
    );

    let page = BodyPage {
        content: Bytes::from_static(&[0xff, 0x00, b'a']),
        media_type: Some("application/octet-stream; charset=binary".to_owned()),
        requested_range: BodyRange {
            offset: 8_192,
            length: 4_096,
        },
        actual_range: BodyRange {
            offset: 8_192,
            length: 3,
        },
        total_bytes: 8_195,
        next_offset: None,
        source: BodyPageSource {
            stream: BodyStreamState::Failed,
            observed_bytes: 9_000,
            retained_bytes: 8_195,
            truncated: true,
            truncation_reason: Some(BodyPreviewLimit::PerBodyLimit),
            decoded_encoding_chain: vec!["gzip".to_owned(), "br".to_owned()],
            decoded_output_limited: false,
        },
    };
    let value = serde_json::to_value(&page).expect("body page JSON");
    assert_eq!(value["content"], json!("/wBh"));
    assert_eq!(
        value["requested_range"],
        json!({"offset": 8192, "length": 4096})
    );
    assert_eq!(value["actual_range"], json!({"offset": 8192, "length": 3}));
    assert_eq!(value["source"]["stream"], json!("failed"));
    assert_eq!(
        value["source"]["truncation_reason"],
        json!("per_body_limit")
    );
    assert_eq!(
        value["source"]["decoded_encoding_chain"],
        json!(["gzip", "br"])
    );
    assert_eq!(
        serde_json::from_value::<BodyPage>(value).expect("body page round trip"),
        page
    );
}

#[test]
fn body_representations_and_fixed_limits_match_the_public_contract() {
    assert_eq!(
        serde_json::to_value(BodyRepresentation::Raw).unwrap(),
        json!("raw")
    );
    assert_eq!(
        serde_json::to_value(BodyRepresentation::Decoded).unwrap(),
        json!("decoded")
    );
    assert_eq!(DEFAULT_BODY_PAGE_LENGTH, 8 * 1_024);
    assert_eq!(MAX_BODY_PAGE_LENGTH, 64 * 1_024);
    assert_eq!(MAX_DECODED_CONTENT_BYTES, 16 * 1_024 * 1_024);
    assert_eq!(MAX_TERMINAL_DECODED_CACHE_BYTES, 32 * 1_024 * 1_024);
}

#[test]
fn body_request_validation_rejects_zero_and_oversized_lengths_with_stable_details() {
    for (length, expected_details) in [
        (0, json!({"field": "length", "minimum": 1, "received": 0})),
        (
            MAX_BODY_PAGE_LENGTH + 1,
            json!({
                "field": "length",
                "maximum": MAX_BODY_PAGE_LENGTH,
                "received": MAX_BODY_PAGE_LENGTH + 1
            }),
        ),
    ] {
        let error = BodyContentRequest {
            capture_id: CaptureSequence::new(7),
            capture_revision: 3,
            side: BodySide::Request,
            representation: BodyRepresentation::Raw,
            offset: 0,
            length,
        }
        .validate()
        .expect_err("invalid page length");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
        assert!(!error.retryable);
        assert_eq!(error.details, expected_details);
    }
}

#[test]
fn body_wire_objects_reject_unknown_fields() {
    for value in [
        json!({
            "capture_id": 1,
            "capture_revision": 1,
            "side": "request",
            "representation": "raw",
            "offset": 0,
            "length": 8192,
            "unexpected": true
        }),
        json!({
            "offset": 0,
            "length": 1,
            "unexpected": true
        }),
    ] {
        if value.get("capture_id").is_some() {
            assert!(serde_json::from_value::<BodyContentRequest>(value).is_err());
        } else {
            assert!(serde_json::from_value::<BodyRange>(value).is_err());
        }
    }
    assert!(
        serde_json::from_value::<BodyPage>(json!({
            "content": "",
            "media_type": null,
            "requested_range": {"offset": 0, "length": 1},
            "actual_range": {"offset": 0, "length": 0},
            "total_bytes": 0,
            "next_offset": null,
            "source": {
                "stream": "complete",
                "observed_bytes": 0,
                "retained_bytes": 0,
                "truncated": false,
                "truncation_reason": null,
                "decoded_encoding_chain": [],
                "decoded_output_limited": false
            },
            "unexpected": true
        }))
        .is_err()
    );
}
