use super::{
    CaptureDetail, CaptureHeaderFilter, CaptureLifecycle, CaptureQuery, CaptureSearchCursor,
    CaptureStatusFilter, CaptureTextFilter, CompiledCaptureQuery, MappingPath,
    classify_lifecycle, classify_mapping, match_capture_page,
};
use crate::{
    capture::{
        BodyStreamState, CaptureRecord, CaptureSequence, CaptureSnapshot, CaptureSnapshotMode,
        CapturedExchange,
    },
    control_rpc::protocol::ControlErrorCode,
};
use chrono::{TimeZone, Utc};
use hyper::Method;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn snapshot(
    sequence: u64,
    method: Method,
    original_url: &str,
    effective_url: &str,
    local_path: Option<&str>,
    status: Option<u16>,
    request_headers: Vec<(&str, &str)>,
    response_headers: Vec<(&str, &str)>,
) -> CaptureSnapshot {
    let record = CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method,
        uri: original_url.to_owned(),
        mapped_uri: (original_url != effective_url).then(|| effective_url.to_owned()),
        local_path: local_path.map(str::to_owned),
        status,
        req_headers: request_headers
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect(),
        res_headers: response_headers
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect(),
        req_body: Some("request-secret-body".to_owned()),
        res_body: Some("response-secret-body".to_owned()),
    });
    record.snapshot(CaptureSnapshotMode::MetadataOnly)
}

fn representative_snapshot() -> CaptureSnapshot {
    let mut capture = snapshot(
        27,
        Method::PATCH,
        "https://origin.example/straße?q=1",
        "https://remote.example/straße?q=1",
        Some("/tmp/local.json"),
        Some(206),
        vec![
            ("X-Duplicate", "first"),
            ("x-duplicate", "Straße second"),
            ("X-Trace", "trace-27"),
        ],
        vec![("Content-Type", "application/json"), ("X-Reply", "Grüße")],
    );
    capture.timing.started_at = Utc
        .with_ymd_and_hms(2026, 8, 24, 12, 30, 0)
        .single()
        .expect("fixed UTC time");
    capture
}

fn query() -> CaptureQuery {
    CaptureQuery::default()
}

#[test]
fn mapping_classification_covers_all_four_paths() {
    let cases = [
        ("https://a/x", "https://a/x", None, MappingPath::Unmapped),
        (
            "https://a/x",
            "https://b/x",
            None,
            MappingPath::RemoteOnly,
        ),
        (
            "https://a/x",
            "https://a/x",
            Some("/tmp/x"),
            MappingPath::LocalOnly,
        ),
        (
            "https://a/x",
            "https://b/x",
            Some("/tmp/x"),
            MappingPath::RemoteThenLocal,
        ),
    ];

    for (original, effective, local, expected) in cases {
        let capture = snapshot(
            1,
            Method::GET,
            original,
            effective,
            local,
            Some(200),
            vec![],
            vec![],
        );
        assert_eq!(classify_mapping(&capture), expected);
    }
}

#[test]
fn lifecycle_precedence_is_failed_then_cancelled_then_complete_then_live() {
    let mut capture = representative_snapshot();
    capture.request_body.status.stream = BodyStreamState::Streaming;
    capture.response_body.status.stream = BodyStreamState::Pending;
    assert_eq!(classify_lifecycle(&capture), CaptureLifecycle::Live);

    capture.request_body.status.stream = BodyStreamState::Complete;
    capture.response_body.status.stream = BodyStreamState::Complete;
    assert_eq!(classify_lifecycle(&capture), CaptureLifecycle::Complete);

    capture.request_body.status.stream = BodyStreamState::Cancelled;
    assert_eq!(classify_lifecycle(&capture), CaptureLifecycle::Cancelled);

    capture.response_body.status.stream = BodyStreamState::Failed;
    assert_eq!(
        classify_lifecycle(&capture),
        CaptureLifecycle::Failed,
        "a failed side wins over a cancelled side"
    );
}

#[test]
fn all_capture_filters_are_and_combined_with_explicit_text_modes() {
    let capture = representative_snapshot();
    let mut all = query();
    all.method = Some("patch".to_owned());
    all.original_url = Some(CaptureTextFilter::Substring("ORIGIN.EXAMPLE/STRASSE".to_owned()));
    all.effective_url = Some(CaptureTextFilter::Glob(
        "https://remote.example/*?q=1".to_owned(),
    ));
    all.status = Some(CaptureStatusFilter {
        exact: None,
        minimum: Some(200),
        maximum: Some(299),
    });
    all.header = Some(CaptureHeaderFilter {
        name: "X-DUPLICATE".to_owned(),
        value: Some("STRASSE SECOND".to_owned()),
    });
    all.mapping_path = Some(MappingPath::RemoteThenLocal);
    all.lifecycle = Some(CaptureLifecycle::Complete);
    all.started_at_min = Some("2026-08-24T12:30:00Z".to_owned());
    all.started_at_max = Some("2026-08-24T12:30:00Z".to_owned());
    all.sequence_min = Some(27);
    all.sequence_max = Some(27);
    all.text = Some("GRÜSSE".to_owned());

    let compiled = CompiledCaptureQuery::compile(all.clone()).expect("valid query");
    assert!(
        compiled
            .matches(&capture, &CancellationToken::new())
            .expect("match")
    );

    let mismatches = [
        ("method", {
            let mut q = all.clone();
            q.method = Some("GET".to_owned());
            q
        }),
        ("original URL", {
            let mut q = all.clone();
            q.original_url = Some(CaptureTextFilter::Substring("elsewhere".to_owned()));
            q
        }),
        ("effective URL", {
            let mut q = all.clone();
            q.effective_url = Some(CaptureTextFilter::Glob("https://other/*".to_owned()));
            q
        }),
        ("status", {
            let mut q = all.clone();
            q.status = Some(CaptureStatusFilter {
                exact: Some(201),
                minimum: None,
                maximum: None,
            });
            q
        }),
        ("header", {
            let mut q = all.clone();
            q.header = Some(CaptureHeaderFilter {
                name: "x-duplicate".to_owned(),
                value: Some("missing".to_owned()),
            });
            q
        }),
        ("mapping", {
            let mut q = all.clone();
            q.mapping_path = Some(MappingPath::RemoteOnly);
            q
        }),
        ("lifecycle", {
            let mut q = all.clone();
            q.lifecycle = Some(CaptureLifecycle::Live);
            q
        }),
        ("UTC lower bound", {
            let mut q = all.clone();
            q.started_at_min = Some("2026-08-24T12:30:01Z".to_owned());
            q
        }),
        ("sequence upper bound", {
            let mut q = all.clone();
            q.sequence_max = Some(26);
            q
        }),
        ("Unicode text", {
            let mut q = all;
            q.text = Some("absent".to_owned());
            q
        }),
    ];

    for (name, mismatch) in mismatches {
        let compiled = CompiledCaptureQuery::compile(mismatch).expect("valid mismatch query");
        assert!(
            !compiled
                .matches(&capture, &CancellationToken::new())
                .expect("match"),
            "{name} must participate in the AND"
        );
    }
}

#[test]
fn exact_status_is_distinct_from_inclusive_status_range() {
    let capture = representative_snapshot();
    for filter in [
        CaptureStatusFilter {
            exact: Some(206),
            minimum: None,
            maximum: None,
        },
        CaptureStatusFilter {
            exact: None,
            minimum: Some(206),
            maximum: Some(206),
        },
    ] {
        let mut q = query();
        q.status = Some(filter);
        assert!(
            CompiledCaptureQuery::compile(q)
                .expect("valid status")
                .matches(&capture, &CancellationToken::new())
                .expect("match")
        );
    }
}

#[test]
fn response_dependent_filters_do_not_match_before_response_metadata_exists() {
    let capture = snapshot(
        3,
        Method::GET,
        "https://example.test/live",
        "https://example.test/live",
        None,
        None,
        vec![("X-Request", "present")],
        vec![],
    );
    let filters = [
        {
            let mut q = query();
            q.status = Some(CaptureStatusFilter {
                exact: Some(200),
                minimum: None,
                maximum: None,
            });
            q
        },
        {
            let mut q = query();
            q.header = Some(CaptureHeaderFilter {
                name: "x-response-only".to_owned(),
                value: None,
            });
            q
        },
    ];

    for q in filters {
        assert!(
            !CompiledCaptureQuery::compile(q)
                .expect("valid query")
                .matches(&capture, &CancellationToken::new())
                .expect("match")
        );
    }
}

#[test]
fn invalid_globs_ranges_times_and_page_limits_reject_before_matching() {
    let invalid = [
        {
            let mut q = query();
            q.original_url = Some(CaptureTextFilter::Glob("[unterminated".to_owned()));
            q
        },
        {
            let mut q = query();
            q.status = Some(CaptureStatusFilter {
                exact: Some(200),
                minimum: Some(100),
                maximum: None,
            });
            q
        },
        {
            let mut q = query();
            q.status = Some(CaptureStatusFilter {
                exact: None,
                minimum: Some(500),
                maximum: Some(400),
            });
            q
        },
        {
            let mut q = query();
            q.started_at_min = Some("not-a-time".to_owned());
            q
        },
        {
            let mut q = query();
            q.started_at_min = Some("2026-08-25T00:00:00Z".to_owned());
            q.started_at_max = Some("2026-08-24T00:00:00Z".to_owned());
            q
        },
        {
            let mut q = query();
            q.sequence_min = Some(9);
            q.sequence_max = Some(8);
            q
        },
    ];

    for q in invalid {
        let error = CompiledCaptureQuery::compile(q).expect_err("invalid query");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
    for limit in [0, 101] {
        let error = match_capture_page(
            &[],
            &CompiledCaptureQuery::compile(query()).expect("empty query"),
            None,
            limit,
            &CancellationToken::new(),
        )
        .expect_err("invalid page size");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn mapping_lifecycle_and_text_modes_serialize_as_stable_snake_case() {
    assert_eq!(
        serde_json::to_value(MappingPath::RemoteThenLocal).expect("mapping JSON"),
        json!("remote_then_local")
    );
    assert_eq!(
        serde_json::to_value(CaptureLifecycle::Cancelled).expect("lifecycle JSON"),
        json!("cancelled")
    );
    assert_eq!(
        serde_json::to_value(CaptureTextFilter::Glob("/v?/items/*".to_owned()))
            .expect("filter JSON"),
        json!({"mode": "glob", "value": "/v?/items/*"})
    );
    assert!(
        serde_json::from_value::<CaptureQuery>(json!({"mapping_path": "remote-local"})).is_err()
    );
    assert!(
        serde_json::from_value::<CaptureQuery>(json!({
            "original_url": {"mode": "heuristic", "value": "*"}
        }))
        .is_err()
    );
}

#[test]
fn sequence_cursor_pagination_is_newest_first_and_stable_across_new_arrivals() {
    let make = |last: u64| {
        (1..=last)
            .map(|sequence| {
                snapshot(
                    sequence,
                    Method::GET,
                    &format!("https://example.test/{sequence}"),
                    &format!("https://example.test/{sequence}"),
                    None,
                    Some(200),
                    vec![],
                    vec![],
                )
            })
            .collect::<Vec<_>>()
    };
    let compiled = CompiledCaptureQuery::compile(query()).expect("empty query");
    let first = match_capture_page(
        &make(40),
        &compiled,
        None,
        10,
        &CancellationToken::new(),
    )
    .expect("first page");
    assert_eq!(
        first
            .captures
            .iter()
            .map(|capture| capture.capture_sequence.value())
            .collect::<Vec<_>>(),
        (31..=40).rev().collect::<Vec<_>>()
    );
    assert_eq!(
        first.next_cursor,
        Some(CaptureSearchCursor::new(CaptureSequence::new(30)))
    );

    let second = match_capture_page(
        &make(41),
        &compiled,
        first.next_cursor,
        10,
        &CancellationToken::new(),
    )
    .expect("second page");
    assert_eq!(
        second
            .captures
            .iter()
            .map(|capture| capture.capture_sequence.value())
            .collect::<Vec<_>>(),
        (21..=30).rev().collect::<Vec<_>>()
    );
}

#[test]
fn omitted_page_limit_defaults_to_twenty_and_explicit_limit_caps_at_one_hundred() {
    assert_eq!(super::normalize_capture_page_limit(None).expect("default"), 20);
    assert_eq!(
        super::normalize_capture_page_limit(Some(100)).expect("maximum"),
        100
    );
    for limit in [Some(0), Some(101)] {
        assert_eq!(
            super::normalize_capture_page_limit(limit)
                .expect_err("out of range")
                .code,
            ControlErrorCode::InvalidArgument
        );
    }
}

#[test]
fn compact_capture_serialization_excludes_headers_and_body_payloads() {
    let page = match_capture_page(
        &[representative_snapshot()],
        &CompiledCaptureQuery::compile(query()).expect("empty query"),
        None,
        20,
        &CancellationToken::new(),
    )
    .expect("page");
    let encoded = serde_json::to_string(&page.captures[0]).expect("compact JSON");
    assert!(!encoded.contains("first"));
    assert!(!encoded.contains("Straße second"));
    assert!(!encoded.contains("secret-body"));
    let value: Value = serde_json::from_str(&encoded).expect("compact value");
    assert!(value.get("request_headers").is_none());
    assert!(value.get("response_headers").is_none());
    assert!(value.get("request_body").is_some());
    assert!(value.get("response_body").is_some());
    assert!(value["request_body"].get("payload").is_none());
    assert!(value["response_body"].get("payload").is_none());
}

#[test]
fn capture_detail_preserves_duplicate_header_order_without_body_bytes() {
    let detail = CaptureDetail::from_snapshot(&representative_snapshot());
    let value = serde_json::to_value(detail).expect("detail JSON");
    assert_eq!(
        value["request_headers"],
        json!([
            {"name": "X-Duplicate", "value": "first"},
            {"name": "x-duplicate", "value": "Straße second"},
            {"name": "X-Trace", "value": "trace-27"}
        ])
    );
    let encoded = serde_json::to_string(&value).expect("detail text");
    assert!(!encoded.contains("request-secret-body"));
    assert!(!encoded.contains("response-secret-body"));
    assert!(value["request_body"].get("payload").is_none());
    assert!(value["response_body"].get("payload").is_none());
}

#[test]
fn maximum_retained_header_matching_observes_cancellation_within_one_scan_quantum() {
    const MAX_RETAINED_HEADER: usize = 256 * 1024;
    let capture = snapshot(
        88,
        Method::GET,
        "https://example.test/large",
        "https://example.test/large",
        None,
        Some(200),
        vec![("X-Large", &"ß".repeat(MAX_RETAINED_HEADER / 2))],
        vec![],
    );
    let mut q = query();
    q.text = Some("not-present".to_owned());
    let compiled = CompiledCaptureQuery::compile(q).expect("query");
    let cancelled = CancellationToken::new();
    let cancellation = cancelled.clone();
    let mut first_observed = None;

    let error = compiled
        .matches_with_scan_hook(&capture, &cancelled, |bytes_scanned| {
            first_observed.get_or_insert(bytes_scanned);
            cancellation.cancel();
        })
        .expect_err("scan cancellation");

    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert!(
        first_observed.expect("scan hook") <= 4 * 1024,
        "matching must check cancellation at least once per 4 KiB"
    );
}
