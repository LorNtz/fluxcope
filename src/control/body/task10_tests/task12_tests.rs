use std::{
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use serde_json::json;

use super::super::{
    ExtractSelector, SelectionResourceRequest, extract_capture_body, page_selected_representation,
    parse_selection_resource_uri, search_capture_body,
};
use crate::{
    capture::{BodySide, CaptureSequence},
    control::json_walk::JsonPointer,
    control_rpc::protocol::ControlErrorCode,
    instance::RunId,
};

const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const DECODED_URI: &str = "wirelens://127.0.0.1:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/content/decoded";
const RAW_URI: &str = "wirelens://127.0.0.1:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/content/raw";
const JSON_SELECTION_URI: &str = "wirelens://127.0.0.1:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/extract/json-pointer?pointer=%2Fvalue";

fn not_cancelled() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn search_capture_body_preserves_unicode_ranges_and_aligned_context() {
    let result = search_capture_body(
        "éStraße雪".as_bytes(),
        "STRASSE",
        10,
        3,
        DECODED_URI,
        RAW_URI,
        &not_cancelled(),
    )
    .expect("text search");

    assert_eq!(result.total_matches, 1);
    assert_eq!(result.omitted_matches, 0);
    let found = &result.matches[0];
    assert_eq!(
        (found.original_range.offset, found.original_range.length),
        (2, 7)
    );
    assert_eq!(found.matched, "Straße");
    assert_eq!(found.context_before, "é");
    assert_eq!(found.context_after, "雪");
    assert_eq!(
        found.resource_uri,
        format!("{DECODED_URI}?offset=0&length=12")
    );
}

#[test]
fn search_capture_body_counts_overlaps_after_retained_limit() {
    let result = search_capture_body(b"aaaa", "aa", 2, 1, DECODED_URI, RAW_URI, &not_cancelled())
        .expect("overlapping search");

    assert_eq!((result.total_matches, result.omitted_matches), (3, 1));
    assert_eq!(
        (
            result.matches[0].original_range.offset,
            result.matches[0].original_range.length
        ),
        (0, 2)
    );
    assert_eq!(
        (
            result.matches[0].context_before.as_str(),
            result.matches[0].context_after.as_str()
        ),
        ("", "a")
    );
    assert_eq!(
        (
            result.matches[1].original_range.offset,
            result.matches[1].original_range.length
        ),
        (1, 2)
    );
    assert_eq!(
        (
            result.matches[1].context_before.as_str(),
            result.matches[1].context_after.as_str()
        ),
        ("a", "a")
    );
}

#[test]
fn search_capture_body_returns_only_a_raw_uri_for_binary_input() {
    let error = search_capture_body(
        &[0xff, 0x00, b's', b'e', b'c', b'r', b'e', b't'],
        "secret",
        10,
        3,
        DECODED_URI,
        RAW_URI,
        &not_cancelled(),
    )
    .expect_err("binary body is not searchable text");

    assert_eq!(error.code, ControlErrorCode::BodyNotTextual);
    assert_eq!(error.details, json!({"raw_content_uri": RAW_URI}));
    assert!(
        !serde_json::to_string(&error)
            .expect("safe error")
            .contains("secret")
    );
}

#[test]
fn extract_capture_body_returns_compact_json_for_root_scalar_object_and_array() {
    let input = br#" { "a/b" : 1, "object" : { "z" : true }, "array" : [ 2, 3 ] } "#;
    for (pointer, expected) in [
        ("", r#"{"a/b":1,"object":{"z":true},"array":[2,3]}"#),
        ("/a~1b", "1"),
        ("/object", r#"{"z":true}"#),
        ("/array", "[2,3]"),
    ] {
        let result = extract_capture_body(
            input,
            Some("application/json; charset=utf-8"),
            &ExtractSelector::JsonPointer {
                pointer: JsonPointer::parse(pointer).expect("pointer"),
            },
            JSON_SELECTION_URI,
            &not_cancelled(),
        )
        .expect("JSON extraction");
        assert_eq!(result.inline.as_deref(), Some(expected));
        assert_eq!(result.selected_bytes, expected.len());
        assert_eq!(result.resource_uri, None);
    }
}

#[test]
fn extract_capture_body_json_miss_exposes_only_prefix_and_child_hints() {
    let error = extract_capture_body(
        br#"{"users":[{"name":"private","roles":["admin"]}]}"#,
        Some("application/json"),
        &ExtractSelector::JsonPointer {
            pointer: JsonPointer::parse("/users/0/missing").expect("pointer"),
        },
        JSON_SELECTION_URI,
        &not_cancelled(),
    )
    .expect_err("missing pointer");

    assert_eq!(error.code, ControlErrorCode::NotFound);
    assert_eq!(
        error.details,
        json!({
            "longest_valid_prefix": "/users/0",
            "next": [
                {"segment": "name", "value_type": "string"},
                {"segment": "roles", "value_type": "array"}
            ]
        })
    );
    let wire = serde_json::to_string(&error).expect("safe error");
    assert!(!wire.contains("private"));
    assert!(!wire.contains("admin"));
}

#[test]
fn extract_capture_body_preserves_repeated_form_utf8_values_in_order() {
    let result = extract_capture_body(
        b"city=%E9%9B%AA&ignored=private&city=&city=Stra%C3%9Fe&city=a+b",
        Some("application/x-www-form-urlencoded"),
        &ExtractSelector::FormField { key: "city".to_owned() },
        "wirelens://127.0.0.1:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/extract/form-field?key=city",
        &not_cancelled(),
    )
    .expect("form extraction");

    assert_eq!(
        result.inline.as_deref(),
        Some(r#"["雪","","Straße","a b"]"#)
    );
    assert_eq!(result.selected_bytes, 26);
    assert_eq!(result.resource_uri, None);
}

#[test]
fn extract_capture_body_inlines_4096_bytes_and_pages_4097_bytes() {
    for (payload_bytes, expected_bytes, is_inline) in [(4_094, 4_096, true), (4_095, 4_097, false)]
    {
        let input = format!(r#"{{"value":"{}"}}"#, "x".repeat(payload_bytes));
        let result = extract_capture_body(
            input.as_bytes(),
            Some("application/json"),
            &ExtractSelector::JsonPointer {
                pointer: JsonPointer::parse("/value").expect("pointer"),
            },
            JSON_SELECTION_URI,
            &not_cancelled(),
        )
        .expect("threshold extraction");

        assert_eq!(result.selected_bytes, expected_bytes);
        assert_eq!(result.inline.is_some(), is_inline);
        if is_inline {
            assert_eq!(result.resource_uri, None);
        } else {
            assert_eq!(
                result.resource_uri.as_deref(),
                Some(
                    "wirelens://127.0.0.1:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/extract/json-pointer?pointer=%2Fvalue&offset=0&length=8192"
                )
            );
        }
    }
}

#[test]
fn selection_uri_is_canonical_and_round_trips_ipv6_and_encoded_pointer() {
    let uri = "wirelens://[2001:db8::1]:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/extract/json-pointer?pointer=%2Fa%7E1b%2F%E9%9B%AA&offset=0&length=8192";
    let parsed = parse_selection_resource_uri(uri).expect("canonical selection URI");

    assert_eq!(
        parsed,
        SelectionResourceRequest {
            proxy_endpoint: "[2001:db8::1]:8080".parse().expect("endpoint"),
            run_id: RunId::from_str(RUN_ID).expect("run ID"),
            capture_id: CaptureSequence::new(9),
            capture_revision: 3,
            side: BodySide::Response,
            selector: ExtractSelector::JsonPointer {
                pointer: JsonPointer::parse("/a~1b/雪").expect("pointer")
            },
            offset: 0,
            length: 8_192,
        }
    );
    assert_eq!(parsed.to_uri().expect("selection URI"), uri);
}

#[test]
fn selection_page_rejects_non_utf8_boundary_with_floor_and_ceiling_hints() {
    let error = page_selected_representation(r#""é雪""#.as_bytes(), 2, 3, JSON_SELECTION_URI)
        .expect_err("offset splits a UTF-8 code point");

    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    assert_eq!(
        error.details,
        json!({
            "field": "offset",
            "received": 2,
            "nearest_floor": 1,
            "nearest_ceiling": 3
        })
    );
}

#[test]
fn extraction_rejects_malformed_depth_and_size_limited_json() {
    let selector = ExtractSelector::JsonPointer {
        pointer: JsonPointer::parse("").expect("root pointer"),
    };
    let malformed = extract_capture_body(
        br#"{"private":"value""#,
        Some("application/json"),
        &selector,
        JSON_SELECTION_URI,
        &not_cancelled(),
    )
    .expect_err("malformed JSON");
    assert_eq!(malformed.code, ControlErrorCode::MalformedJson);
    assert!(
        !serde_json::to_string(&malformed)
            .expect("safe malformed error")
            .contains("private")
    );

    let nested = format!("{}0{}", "[".repeat(513), "]".repeat(513));
    let depth = extract_capture_body(
        nested.as_bytes(),
        Some("application/json"),
        &selector,
        JSON_SELECTION_URI,
        &not_cancelled(),
    )
    .expect_err("depth limit");
    assert_eq!(depth.code, ControlErrorCode::JsonDepthLimit);

    let oversized = vec![b' '; super::super::MAX_DECODED_CONTENT_BYTES + 1];
    let size = extract_capture_body(
        &oversized,
        Some("application/json"),
        &selector,
        JSON_SELECTION_URI,
        &not_cancelled(),
    )
    .expect_err("JSON input size limit");
    assert_eq!(size.code, ControlErrorCode::JsonSizeLimit);
}

#[test]
fn extraction_observes_json_and_large_form_field_cancellation() {
    let cancelled = AtomicBool::new(true);
    let json_error = extract_capture_body(
        br#"{"value":"private"}"#,
        Some("application/json"),
        &ExtractSelector::JsonPointer {
            pointer: JsonPointer::parse("/value").expect("pointer"),
        },
        JSON_SELECTION_URI,
        &cancelled,
    )
    .expect_err("cancelled JSON extraction");
    assert_eq!(json_error.code, ControlErrorCode::Cancelled);
    assert!(
        !serde_json::to_string(&json_error)
            .expect("safe cancellation error")
            .contains("private")
    );

    let form = format!("ignored={}&target=value", "x".repeat(15 * 1_024 * 1_024));
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(1));
            cancelled.store(true, Ordering::Relaxed);
        });
        let error = extract_capture_body(
            form.as_bytes(),
            Some("application/x-www-form-urlencoded"),
            &ExtractSelector::FormField {
                key: "target".to_owned(),
            },
            "wirelens://127.0.0.1:8080/runs/AAAAAAAAAAAAAAAAAAAAAA/captures/9/revisions/3/bodies/response/extract/form-field?key=target",
            &cancelled,
        )
        .expect_err("cancellation inside a large form field");
        assert_eq!(error.code, ControlErrorCode::Cancelled);
    });
}

#[test]
fn form_extraction_rejects_wrong_media_type_and_oversized_input() {
    let selector = ExtractSelector::FormField {
        key: "target".to_owned(),
    };
    let wrong_media = extract_capture_body(
        b"target=value",
        Some("text/plain"),
        &selector,
        JSON_SELECTION_URI,
        &not_cancelled(),
    )
    .expect_err("wrong form media type");
    assert_eq!(wrong_media.code, ControlErrorCode::InvalidArgument);

    let oversized = vec![b'x'; super::super::MAX_DECODED_CONTENT_BYTES + 1];
    let size = extract_capture_body(
        &oversized,
        Some("application/x-www-form-urlencoded"),
        &selector,
        JSON_SELECTION_URI,
        &not_cancelled(),
    )
    .expect_err("form input size limit");
    assert_eq!(size.code, ControlErrorCode::JsonSizeLimit);
}
