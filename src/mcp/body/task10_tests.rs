use super::{BodyResourceUri, CONTENT_RESOURCE_TEMPLATE, body_page_resource_contents};
use crate::{
    capture::{BodyPreviewLimit, BodySide, BodyStreamState, CaptureSequence},
    control::body::{
        BodyPage, BodyPageSource, BodyRange, BodyRepresentation, DEFAULT_BODY_PAGE_LENGTH,
        MAX_BODY_PAGE_LENGTH,
    },
    instance::RunId,
};
use hyper::body::Bytes;
use rmcp::model::ResourceContents;
use serde_json::{Value, json};
use std::{net::SocketAddr, str::FromStr};

const RUN: &str = "AAAAAAAAAAAAAAAAAAAAAA";

fn run_id() -> RunId {
    RunId::from_str(RUN).expect("run ID")
}

fn endpoint() -> SocketAddr {
    "127.0.0.1:8989".parse().expect("endpoint")
}

fn uri(
    side: BodySide,
    representation: BodyRepresentation,
    offset: usize,
    length: usize,
) -> BodyResourceUri {
    BodyResourceUri::content(
        endpoint(),
        run_id(),
        CaptureSequence::new(7),
        3,
        side,
        representation,
        offset,
        length,
    )
}

#[test]
fn content_template_is_the_exact_task_ten_rfc6570_template() {
    assert_eq!(
        CONTENT_RESOURCE_TEMPLATE,
        "wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/content/{representation}{?offset,length}"
    );
}

#[test]
fn resource_uri_round_trips_ipv6_endpoint_and_explicit_range_canonically() {
    let value = BodyResourceUri::content(
        "[::1]:8989".parse().expect("IPv6 endpoint"),
        run_id(),
        CaptureSequence::new(0),
        3,
        BodySide::Response,
        BodyRepresentation::Decoded,
        8_192,
        4_096,
    );
    assert_eq!(
        value.as_str(),
        format!(
            "wirelens://[::1]:8989/runs/{RUN}/captures/0/revisions/3/bodies/response/content/decoded?offset=8192&length=4096"
        )
    );
    assert_eq!(
        BodyResourceUri::parse(value.as_str()).expect("parse"),
        value
    );
}

#[test]
fn parser_defaults_only_missing_offset_and_length() {
    let base = format!(
        "wirelens://127.0.0.1:8989/runs/{RUN}/captures/0/revisions/3/bodies/request/content/raw"
    );
    let parsed = BodyResourceUri::parse(&base).expect("defaults");
    assert_eq!(parsed.capture_id(), CaptureSequence::new(0));
    assert_eq!(parsed.offset(), 0);
    assert_eq!(parsed.length(), DEFAULT_BODY_PAGE_LENGTH);

    let offset_only = BodyResourceUri::parse(&format!("{base}?offset=4")).expect("offset only");
    assert_eq!(offset_only.offset(), 4);
    assert_eq!(offset_only.length(), DEFAULT_BODY_PAGE_LENGTH);
    let length_only = BodyResourceUri::parse(&format!("{base}?length=3")).expect("length only");
    assert_eq!(length_only.offset(), 0);
    assert_eq!(length_only.length(), 3);
}

#[test]
fn parser_accepts_the_full_canonical_capture_id_domain_including_zero() {
    for capture_id in [0_u64, 1, u64::MAX] {
        let value = format!(
            "wirelens://127.0.0.1:8989/runs/{RUN}/captures/{capture_id}/revisions/0/bodies/request/content/raw"
        );
        assert_eq!(
            BodyResourceUri::parse(&value)
                .expect("canonical capture ID")
                .capture_id(),
            CaptureSequence::new(capture_id)
        );
    }
}

#[test]
fn parser_rejects_noncanonical_authorities_paths_and_identity_segments() {
    let valid = format!(
        "wirelens://127.0.0.1:8989/runs/{RUN}/captures/7/revisions/3/bodies/response/content/decoded"
    );
    let invalid = [
        valid.replacen("wirelens://", "http://", 1),
        valid.replacen("127.0.0.1:8989", "user@127.0.0.1:8989", 1),
        valid.replacen("127.0.0.1:8989", "localhost:8989", 1),
        valid.replacen("127.0.0.1:8989", "127.0.0.1", 1),
        valid.replacen("127.0.0.1:8989", "::1:8989", 1),
        valid.replacen("/runs/", "//runs/", 1),
        valid.replacen("/runs/", "/extra/runs/", 1),
        valid.replacen("/content/decoded", "/content/decoded/extra", 1),
        valid.replacen(&format!("/runs/{RUN}"), "", 1),
        valid.replacen("/captures/7/", "/captures//", 1),
        valid.replacen("/content/decoded", "/content/", 1),
        valid.replacen("/captures/7/", "/captures/../", 1),
        valid.replacen("/captures/7/", "/captures/%2e%2e/", 1),
        valid.replacen("/captures/7/", "/ignored/../captures/7/", 1),
        valid.replacen("/captures/7/", "/ignored/%2e%2e/captures/7/", 1),
        valid.replacen(RUN, "not-a-run-id", 1),
        valid.replacen("/captures/7/", "/captures/+7/", 1),
        valid.replacen("/captures/7/", "/captures/-7/", 1),
        valid.replacen("/captures/7/", "/captures/07/", 1),
        valid.replacen("/captures/7/", "/captures/18446744073709551616/", 1),
        valid.replacen("/revisions/3/", "/revisions/-1/", 1),
        valid.replacen("/revisions/3/", "/revisions/03/", 1),
        valid.replacen("/bodies/response/", "/bodies/res/", 1),
        valid.replacen("/content/decoded", "/content/display", 1),
    ];
    for candidate in invalid {
        assert!(
            BodyResourceUri::parse(&candidate).is_err(),
            "must reject {candidate}"
        );
    }
}

#[test]
fn parser_rejects_fragments_unknown_duplicate_malformed_and_invalid_query_values() {
    let base = format!(
        "wirelens://127.0.0.1:8989/runs/{RUN}/captures/7/revisions/3/bodies/response/content/decoded"
    );
    let invalid = [
        format!("{base}#fragment"),
        format!("{base}?cursor=1"),
        format!("{base}?offset=1&offset=2"),
        format!("{base}?length=1&length=2"),
        format!("{base}?offset=%"),
        format!("{base}?offset=%GG"),
        format!("{base}?offset=-1"),
        format!("{base}?offset=+1"),
        format!("{base}?offset=01"),
        format!("{base}?offset=18446744073709551616"),
        format!("{base}?length=0"),
        format!("{base}?length={}", MAX_BODY_PAGE_LENGTH + 1),
        format!("{base}?length=18446744073709551616"),
        format!("{base}?length=1&"),
    ];
    for candidate in invalid {
        assert!(
            BodyResourceUri::parse(&candidate).is_err(),
            "must reject {candidate}"
        );
    }
}

#[test]
fn next_uri_preserves_exact_generation_side_and_representation() {
    let first = BodyResourceUri::content(
        "[2001:db8::1]:443".parse().unwrap(),
        run_id(),
        CaptureSequence::new(0),
        u64::MAX,
        BodySide::Request,
        BodyRepresentation::Raw,
        0,
        8_192,
    );
    let next = first.next_page(7_777);
    assert_eq!(next.proxy_endpoint(), first.proxy_endpoint());
    assert_eq!(next.run_id(), first.run_id());
    assert_eq!(next.capture_id(), CaptureSequence::new(0));
    assert_eq!(next.capture_revision(), u64::MAX);
    assert_eq!(next.side(), BodySide::Request);
    assert_eq!(next.representation(), BodyRepresentation::Raw);
    assert_eq!(next.offset(), 7_777);
    assert_eq!(next.length(), 8_192);
    assert_eq!(BodyResourceUri::parse(next.as_str()).unwrap(), next);
}

fn page(content: Bytes, representation: BodyRepresentation) -> BodyPage {
    BodyPage {
        content,
        media_type: Some("text/plain; charset=utf-8".to_owned()),
        requested_range: BodyRange {
            offset: 1,
            length: 2,
        },
        actual_range: BodyRange {
            offset: 1,
            length: 2,
        },
        total_bytes: 4,
        next_offset: Some(3),
        source: BodyPageSource {
            stream: BodyStreamState::Failed,
            observed_bytes: 12,
            retained_bytes: 8,
            truncated: true,
            truncation_reason: Some(BodyPreviewLimit::TotalMemoryLimit),
            decoded_encoding_chain: match representation {
                BodyRepresentation::Raw => vec![],
                BodyRepresentation::Decoded => vec!["gzip".to_owned()],
            },
            decoded_output_limited: representation == BodyRepresentation::Decoded,
        },
    }
}

fn contents_json(contents: ResourceContents) -> Value {
    serde_json::to_value(contents).expect("resource contents JSON")
}

#[test]
fn raw_resource_base64_encodes_only_the_already_sliced_page_and_emits_exact_meta() {
    let requested = uri(BodySide::Response, BodyRepresentation::Raw, 1, 2);
    let value = contents_json(
        body_page_resource_contents(
            &requested,
            page(Bytes::from_static(&[0xff, 0x00]), BodyRepresentation::Raw),
        )
        .expect("raw contents"),
    );
    assert_eq!(value["uri"], requested.as_str());
    assert_eq!(value["blob"], json!("/wA="));
    assert!(value.get("text").is_none());
    assert_eq!(value["mimeType"], json!("text/plain; charset=utf-8"));
    assert_eq!(
        value["_meta"]["wirelens"],
        json!({
            "requested_range": {"offset": 1, "length": 2},
            "actual_range": {"offset": 1, "length": 2},
            "total_bytes": 4,
            "next_uri": uri(BodySide::Response, BodyRepresentation::Raw, 3, 2).as_str(),
            "proxy_endpoint": "127.0.0.1:8989",
            "run_id": RUN,
            "capture_id": 7,
            "capture_revision": 3,
            "side": "response",
            "representation": "raw",
            "stream": "failed",
            "observed_bytes": 12,
            "retained_bytes": 8,
            "source_truncated": true,
            "source_truncation_reason": "total_memory_limit",
            "decoded_encoding_chain": [],
            "decoded_output_limited": false
        })
    );
}

#[test]
fn decoded_utf8_is_text_with_retained_mime_and_decoded_binary_is_blob() {
    let requested = uri(BodySide::Request, BodyRepresentation::Decoded, 1, 2);
    let text = contents_json(
        body_page_resource_contents(
            &requested,
            page(Bytes::from_static(b"ok"), BodyRepresentation::Decoded),
        )
        .expect("decoded text"),
    );
    assert_eq!(text["uri"], requested.as_str());
    assert_eq!(text["text"], json!("ok"));
    assert_eq!(text["mimeType"], json!("text/plain; charset=utf-8"));
    assert!(text.get("blob").is_none());
    assert_eq!(
        text["_meta"]["wirelens"]["decoded_encoding_chain"],
        json!(["gzip"])
    );
    assert_eq!(
        text["_meta"]["wirelens"]["decoded_output_limited"],
        json!(true)
    );

    let binary = contents_json(
        body_page_resource_contents(
            &requested,
            page(
                Bytes::from_static(&[0xff, 0x00]),
                BodyRepresentation::Decoded,
            ),
        )
        .expect("decoded binary"),
    );
    assert_eq!(binary["blob"], json!("/wA="));
    assert!(binary.get("text").is_none());
}
