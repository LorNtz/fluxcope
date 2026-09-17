//! Narrow public seams used only by Criterion's external benchmark target.

#[cfg(unix)]
mod system;
#[cfg(unix)]
pub use system::*;
mod workloads;
pub use workloads::*;

use crate::{
    app::RequestTreeModel,
    capture::{BodyStreamState, CaptureRecord, CaptureSnapshotMode, CapturedExchange},
    control::{
        body::{BodyPage, BodyPageSource, page_selected_representation, search_capture_body},
        capture_query::{
            CaptureHeaderFilter, CaptureQuery, CaptureTextFilter, CompiledCaptureQuery,
        },
        json_walk::{FieldMatchMode, find_json_pointers, probe_json},
    },
    control_rpc::framing::{RESPONSE_MAX_BYTES, encode_json_frame},
    request_search::{
        SearchJobOutcome, SearchRequest, SearchRequestKey, run_search, start_request_search_service,
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use std::{
    io::Write as _,
    sync::{Arc, atomic::AtomicBool},
};
use tokio_util::sync::CancellationToken;

pub fn ordered_capture_store(captures: Vec<CapturedExchange>) -> usize {
    crate::capture::benchmark_ordered_store(captures)
}

pub fn request_tree_build_and_snapshot(captures: Vec<CapturedExchange>) -> usize {
    crate::app::benchmark_request_tree(captures)
}

pub fn retained_log_join(record_count: usize, record_bytes: usize) -> usize {
    crate::app::benchmark_retained_log_join(record_count, record_bytes)
}

pub fn yaml_semantic_preservation(rule_count: usize) -> usize {
    crate::settings::benchmark_yaml_semantic_preservation(rule_count)
}

pub struct RequestSearchFixture {
    tree: Arc<RequestTreeModel>,
}

pub fn request_search_fixture(captures: Vec<CapturedExchange>) -> RequestSearchFixture {
    let summaries = captures
        .into_iter()
        .map(CaptureRecord::from_completed)
        .map(|capture| capture.summary());
    RequestSearchFixture {
        tree: Arc::new(RequestTreeModel::from_requests(summaries)),
    }
}

pub fn request_tree_search(fixture: &RequestSearchFixture, query: &str) -> usize {
    let request = SearchRequest {
        key: SearchRequestKey {
            generation: 1,
            tree_revision: 1,
        },
        query: Arc::from(query),
        tree: Arc::clone(&fixture.tree),
    };
    match run_search(&request, &AtomicBool::new(false)) {
        SearchJobOutcome::Completed { results, .. } => results.len(),
        SearchJobOutcome::Cancelled { .. } | SearchJobOutcome::Failed { .. } => 0,
    }
}

pub async fn request_search_rapid_supersession(
    fixture: &RequestSearchFixture,
    replacements: usize,
) -> u64 {
    let shutdown = CancellationToken::new();
    let service = start_request_search_service(shutdown.clone());
    let client = service.client;
    let mut results = service.results;
    let task = service.task;
    let replacements = replacements.max(1);
    let search_request = |generation| SearchRequest {
        key: SearchRequestKey {
            generation,
            tree_revision: 1,
        },
        query: Arc::from(format!("query{generation}")),
        tree: Arc::clone(&fixture.tree),
    };

    client.submit(search_request(0));
    tokio::task::yield_now().await;
    for generation in 1..replacements as u64 {
        client.submit(search_request(generation));
    }
    let latest_generation = replacements as u64 - 1;
    loop {
        results
            .changed()
            .await
            .expect("benchmark search service should remain available");
        let outcome = results.borrow_and_update().clone();
        if outcome
            .as_deref()
            .is_some_and(|outcome| outcome.key().generation == latest_generation)
        {
            break;
        }
    }

    shutdown.cancel();
    task.await
        .expect("benchmark search task should join")
        .expect("benchmark search service should stop cleanly");
    latest_generation
}

#[derive(Clone, Copy, Debug)]
pub enum CaptureQueryScenario {
    Absent,
    Sparse,
    FullPage,
    Header,
    Unicode,
}

pub struct CaptureQueryFixture {
    snapshots: Vec<crate::capture::CaptureSnapshot>,
}

pub fn capture_query_fixture(captures: Vec<CapturedExchange>) -> CaptureQueryFixture {
    CaptureQueryFixture {
        snapshots: captures
            .into_iter()
            .map(CaptureRecord::from_completed)
            .map(|record| record.snapshot(CaptureSnapshotMode::MetadataOnly))
            .collect(),
    }
}

pub fn capture_query_search(
    fixture: &CaptureQueryFixture,
    scenario: CaptureQueryScenario,
) -> usize {
    let query = match scenario {
        CaptureQueryScenario::Absent => CaptureQuery {
            original_url: Some(CaptureTextFilter::Substring(
                "definitely-absent-capture-query".to_owned(),
            )),
            ..CaptureQuery::default()
        },
        CaptureQueryScenario::Sparse => CaptureQuery {
            original_url: Some(CaptureTextFilter::Substring("/item/0".to_owned())),
            ..CaptureQuery::default()
        },
        CaptureQueryScenario::FullPage => CaptureQuery {
            method: Some("GET".to_owned()),
            ..CaptureQuery::default()
        },
        CaptureQueryScenario::Header => CaptureQuery {
            header: Some(CaptureHeaderFilter {
                name: "x-benchmark".to_owned(),
                value: Some("needle".to_owned()),
            }),
            ..CaptureQuery::default()
        },
        CaptureQueryScenario::Unicode => CaptureQuery {
            original_url: Some(CaptureTextFilter::Substring("strasse".to_owned())),
            ..CaptureQuery::default()
        },
    };
    let compiled = CompiledCaptureQuery::compile(query).expect("benchmark query should compile");
    let cancelled = CancellationToken::new();
    fixture
        .snapshots
        .iter()
        .rev()
        .filter(|snapshot| {
            compiled
                .matches(snapshot, &cancelled)
                .expect("benchmark capture match should succeed")
        })
        .take(100)
        .count()
}

pub struct BodyFixture {
    bytes: Vec<u8>,
}

pub fn body_fixture(exchange: CapturedExchange) -> BodyFixture {
    BodyFixture {
        bytes: CaptureRecord::from_completed(exchange)
            .snapshot(CaptureSnapshotMode::WithBodyPreviews)
            .response_body
            .preview
            .flatten(),
    }
}

pub fn search_body(fixture: &BodyFixture, query: &str, context_bytes: usize) -> usize {
    search_capture_body(
        &fixture.bytes,
        query,
        20,
        context_bytes,
        "fluxcope://benchmark/decoded",
        "fluxcope://benchmark/raw",
        &AtomicBool::new(false),
    )
    .expect("benchmark body search should succeed")
    .matches
    .len()
}

pub fn private_body_page_relay(fixture: &BodyFixture, offset: usize, length: usize) -> usize {
    let window =
        page_selected_representation(&fixture.bytes, offset, length, "fluxcope://benchmark/raw")
            .expect("benchmark body page should be valid");
    let page = BodyPage {
        content: window.content,
        media_type: Some("application/octet-stream".to_owned()),
        requested_range: window.requested_range,
        actual_range: window.actual_range,
        total_bytes: window.total_bytes,
        next_offset: window.next_offset,
        source: BodyPageSource {
            stream: BodyStreamState::Complete,
            observed_bytes: fixture.bytes.len() as u64,
            retained_bytes: fixture.bytes.len(),
            truncated: false,
            truncation_reason: None,
            decoded_encoding_chain: Vec::new(),
            decoded_output_limited: false,
        },
    };
    encode_json_frame(&page, RESPONSE_MAX_BYTES)
        .expect("benchmark body page should serialize")
        .len()
}

pub struct DecodedBodyFixture {
    preview: crate::capture::CapturedBodyPreview,
    headers: crate::capture::CapturedHeaders,
}

pub fn gzip_body_fixture(uncompressed_bytes: usize) -> DecodedBodyFixture {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let content = vec![b'x'; uncompressed_bytes];
    encoder
        .write_all(&content)
        .expect("benchmark gzip content should encode");
    let compressed = encoder.finish().expect("benchmark gzip should finish");
    DecodedBodyFixture {
        preview: crate::capture::CapturedBodyPreview::unbudgeted(bytes::Bytes::from(compressed)),
        headers: crate::capture::CapturedHeaders::unbudgeted(Arc::from([
            ("content-encoding".to_owned(), "gzip".to_owned()),
            ("content-type".to_owned(), "text/plain".to_owned()),
        ])),
    }
}

pub fn decode_gzip_body(fixture: &DecodedBodyFixture) -> usize {
    crate::capture::decode_content_bytes(
        &fixture.preview,
        &fixture.headers,
        &crate::capture::ContentDecodePolicy::default(),
        &AtomicBool::new(false),
    )
    .expect("benchmark gzip body should decode")
    .bytes
    .len()
}

pub fn decode_page_and_base64(fixture: &DecodedBodyFixture, offset: usize, length: usize) -> usize {
    let decoded = crate::capture::decode_content_bytes(
        &fixture.preview,
        &fixture.headers,
        &crate::capture::ContentDecodePolicy::default(),
        &AtomicBool::new(false),
    )
    .expect("benchmark gzip body should decode");
    let page = page_selected_representation(
        &decoded.bytes,
        offset,
        length,
        "fluxcope://benchmark/decoded",
    )
    .expect("benchmark decoded page should be valid");
    BASE64_STANDARD.encode(page.content).len()
}

pub struct JsonFixture {
    bytes: Vec<u8>,
}

pub fn json_fixture(bytes: Vec<u8>) -> JsonFixture {
    JsonFixture { bytes }
}

pub fn find_json_fields(fixture: &JsonFixture, field_name: &str) -> usize {
    find_json_pointers(
        &fixture.bytes,
        field_name,
        FieldMatchMode::Exact,
        20,
        &AtomicBool::new(false),
    )
    .expect("benchmark JSON field discovery should succeed")
    .matches
    .len()
}

pub fn probe_json_pattern(fixture: &JsonFixture, pattern: &str) -> usize {
    let pattern = pattern
        .parse()
        .expect("benchmark JSON pointer pattern should parse");
    let result = probe_json(&fixture.bytes, &pattern, &AtomicBool::new(false))
        .expect("benchmark JSON pattern probe should succeed");
    result.examples.len().saturating_add(result.match_count)
}
