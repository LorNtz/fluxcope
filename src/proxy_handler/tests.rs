use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;

use super::*;
use crate::{
    capture::{
        BodyStreamState, CapturePolicy, CaptureRecord, CaptureSequence, CaptureSnapshotMode,
    },
    request_policy::RequestPolicy,
    settings::{
        AppSettings, ProxyMapLocalRule, ProxyMapLocalSettings, ProxyMapRemoteRule,
        ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
        RecordingPrefilterPatternSettings, RecordingPrefilterSettings,
    },
};
use chrono::Utc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Harness {
    handler: LogHandler,
    captures: mpsc::Receiver<std::sync::Arc<CaptureRecord>>,
    shutdown: CancellationToken,
    tasks: BodyTaskTracker,
}

impl Harness {
    fn new(request_policy_store: RequestPolicyStore, recording: RecordingState) -> Self {
        let policy = CapturePolicy::default();
        let (tx, captures) = mpsc::channel(policy.queue_capacity);
        let publisher = CapturePublisher::new(tx, policy);
        let shutdown = CancellationToken::new();
        let tasks = BodyTaskTracker::new(shutdown.clone());
        let handler = LogHandler::new(publisher, tasks.clone(), request_policy_store, recording);
        Self {
            handler,
            captures,
            shutdown,
            tasks,
        }
    }

    async fn next_capture(&mut self) -> std::sync::Arc<CaptureRecord> {
        self.captures
            .recv()
            .await
            .expect("capture should be published")
    }

    async fn finish(self) {
        self.shutdown.cancel();
        self.tasks
            .wait_for_shutdown(Duration::from_secs(1))
            .await
            .expect("body tasks should stop");
    }
}

#[tokio::test]
async fn proxy_preserves_both_body_byte_streams_and_records_real_timing_order() {
    let mut harness = Harness::new(RequestPolicyStore::default(), RecordingState::default());
    let request_bytes = b"request-one-request-two-request-three";
    let request_body = Body::wrap_stream(futures::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"request-one-")),
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"request-two-")),
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"request-three")),
    ]));
    let before_start = Utc::now();
    let forwarded_request = match harness
        .handler
        .capture_request(request("POST", "https://example.com/timed", request_body))
        .await
    {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("request should be forwarded"),
    };
    let after_start = Utc::now();
    let forwarded_request_bytes = hudsucker::hyper::body::to_bytes(forwarded_request.into_body())
        .await
        .expect("request body should forward");
    assert_eq!(forwarded_request_bytes.as_ref(), request_bytes);

    tokio::time::sleep(Duration::from_millis(2)).await;
    let response_bytes = b"response-one-response-two-response-three";
    let response_body = Body::wrap_stream(futures::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"response-one-")),
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"response-two-")),
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"response-three")),
    ]));
    let forwarded_response = harness
        .handler
        .capture_response(Response::new(response_body));
    let forwarded_response_bytes = hudsucker::hyper::body::to_bytes(forwarded_response.into_body())
        .await
        .expect("response body should forward");
    assert_eq!(forwarded_response_bytes.as_ref(), response_bytes);

    let record = harness.next_capture().await;
    let snapshot = record.snapshot(CaptureSnapshotMode::WithBodyPreviews);
    let time_to_response = snapshot
        .timing
        .time_to_response
        .expect("response timing should be populated");
    let total_duration = snapshot
        .timing
        .total_duration
        .expect("both terminal body streams should populate total duration");

    assert!(snapshot.timing.started_at >= before_start);
    assert!(snapshot.timing.started_at <= after_start);
    assert!(time_to_response >= Duration::from_millis(2));
    assert!(total_duration >= time_to_response);
    assert_eq!(snapshot.request_body.preview.flatten(), request_bytes);
    assert_eq!(snapshot.response_body.preview.flatten(), response_bytes);
    assert_eq!(
        snapshot.request_body.status.stream,
        BodyStreamState::Complete
    );
    assert_eq!(
        snapshot.response_body.status.stream,
        BodyStreamState::Complete
    );
    harness.finish().await;
}

#[tokio::test]
async fn cloned_handlers_capture_concurrent_requests_independently_in_capture_order() {
    let mut harness = Harness::new(RequestPolicyStore::default(), RecordingState::default());
    let mut slow_handler = harness.handler.clone();
    let mut fast_handler = harness.handler.clone();

    let slow_request = forward_request(&mut slow_handler, "https://example.com/slow").await;
    let fast_request = forward_request(&mut fast_handler, "https://example.com/fast").await;
    consume_request_body(slow_request).await;
    consume_request_body(fast_request).await;

    let fast_response = fast_handler.capture_response(test_response(200, "fast body"));
    let slow_response = slow_handler.capture_response(test_response(201, "slow body"));
    consume_response_body(fast_response).await;
    consume_response_body(slow_response).await;

    let slow = harness.next_capture().await;
    let fast = harness.next_capture().await;
    assert_eq!(slow.sequence(), CaptureSequence::new(0));
    assert_eq!(
        slow.summary().request.original_uri,
        "https://example.com/slow"
    );
    assert_eq!(
        slow.summary().response.as_ref().map(|res| res.status),
        Some(201)
    );
    assert_eq!(
        slow.body_preview(BodySide::Response).flatten(),
        b"slow body"
    );
    assert_eq!(fast.sequence(), CaptureSequence::new(1));
    assert_eq!(
        fast.summary().request.original_uri,
        "https://example.com/fast"
    );
    assert_eq!(
        fast.summary().response.as_ref().map(|res| res.status),
        Some(200)
    );
    assert_eq!(
        fast.body_preview(BodySide::Response).flatten(),
        b"fast body"
    );
    harness.finish().await;
}

#[tokio::test]
async fn remote_mapping_rewrites_forwarded_uri_but_captures_both_uris() {
    let mut harness = Harness::new(
        request_policy_store(
            vec![remote_rule("https://a.com", "http://b.test.com")],
            vec![],
        ),
        RecordingState::default(),
    );
    let request = request("GET", "https://a.com/some/api?x=1", Body::empty());

    let forwarded = match harness.handler.capture_request(request).await {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("remote mapping should forward request"),
    };
    assert_eq!(
        forwarded.uri().to_string(),
        "http://b.test.com/some/api?x=1"
    );
    consume_request_body(forwarded).await;
    consume_response_body(harness.handler.capture_response(test_response(200, "ok"))).await;

    let capture = harness.next_capture().await;
    let summary = capture.summary();
    assert_eq!(summary.request.original_uri, "https://a.com/some/api?x=1");
    assert_eq!(
        summary.request.mapped_uri(),
        Some("http://b.test.com/some/api?x=1")
    );
    assert!(summary.request.local_path.is_none());
    harness.finish().await;
}

#[tokio::test]
async fn first_response_chunk_is_forwarded_before_source_eof() {
    let mut harness = Harness::new(RequestPolicyStore::default(), RecordingState::default());
    let forwarded = forward_request(&mut harness.handler, "https://example.com/").await;
    consume_request_body(forwarded).await;
    let (mut source, body) = body_channel();
    let mut response = harness.handler.capture_response(Response::new(body));
    source
        .send_data(Bytes::from_static(b"first"))
        .await
        .expect("source should accept first chunk");

    use http_body_util::BodyExt as _;
    let first = tokio::time::timeout(Duration::from_secs(1), response.body_mut().frame())
        .await
        .expect("forwarded chunk must not wait for EOF")
        .expect("response should contain a chunk")
        .expect("chunk should be forwarded");
    assert_eq!(first.into_data().expect("data frame").as_ref(), b"first");
    drop(source);
    drop(response);
    harness.finish().await;
}

#[tokio::test]
async fn recording_off_preserves_mapping_without_publishing_capture() {
    let mut harness = Harness::new(
        request_policy_store(
            vec![remote_rule("https://a.com", "http://b.test.com")],
            vec![],
        ),
        RecordingState::new(false),
    );
    let request = request("POST", "https://a.com/some/api?x=1", Body::from("payload"));
    let forwarded = match harness.handler.capture_request(request).await {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("remote mapping should forward request"),
    };
    assert_eq!(
        forwarded.uri().to_string(),
        "http://b.test.com/some/api?x=1"
    );
    assert_eq!(
        crate::capture::body_bytes(forwarded.into_body())
            .await
            .expect("body should forward")
            .as_ref(),
        b"payload"
    );
    assert!(harness.captures.try_recv().is_err());
    harness.finish().await;
}

#[tokio::test]
async fn unmatched_prefilter_request_forwards_bodies_without_publishing_capture() {
    let mut harness = Harness::new(
        prefilter_store(vec![], vec![], vec!["https://wanted.example.com/*"]),
        RecordingState::default(),
    );
    let request = request(
        "POST",
        "https://other.example.com/api?token=visible",
        Body::from("request payload"),
    );

    let forwarded = match harness.handler.capture_request(request).await {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("unmatched request should forward"),
    };
    assert_eq!(
        crate::capture::body_bytes(forwarded.into_body())
            .await
            .expect("request body should forward")
            .as_ref(),
        b"request payload"
    );
    let response = harness
        .handler
        .capture_response(test_response(200, "response payload"));
    assert_eq!(
        crate::capture::body_bytes(response.into_body())
            .await
            .expect("response body should forward")
            .as_ref(),
        b"response payload"
    );
    assert!(harness.captures.try_recv().is_err());
    harness.finish().await;
}

#[tokio::test]
async fn prefilter_matches_explicit_default_https_port_without_changing_captured_url() {
    let mut harness = Harness::new(
        prefilter_store(vec![], vec![], vec!["*://*.example.com/*"]),
        RecordingState::default(),
    );
    let uri = "https://telemetry.example.com:443/api/ministat/x?count=1&e=tech_socket_error";

    let forwarded = match harness
        .handler
        .capture_request(request("POST", uri, Body::empty()))
        .await
    {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("matched request should forward"),
    };
    assert_eq!(forwarded.uri().to_string(), uri);
    consume_request_body(forwarded).await;
    consume_response_body(harness.handler.capture_response(test_response(200, "ok"))).await;

    let capture = harness.next_capture().await;
    assert_eq!(capture.summary().request.display_uri(), uri);
    harness.finish().await;
}

#[tokio::test]
async fn prefilter_matches_effective_url_after_remote_mapping_with_query() {
    let mut harness = Harness::new(
        prefilter_store(
            vec![remote_rule("https://a.com", "http://b.test.com")],
            vec![],
            vec!["http://b.test.com:80/interested*?client=*"],
        ),
        RecordingState::default(),
    );
    let request = request(
        "GET",
        "https://a.com/interested/orders?client=fluxcope",
        Body::empty(),
    );

    let forwarded = match harness.handler.capture_request(request).await {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("remote mapping should forward"),
    };
    assert_eq!(
        forwarded.uri().to_string(),
        "http://b.test.com/interested/orders?client=fluxcope"
    );
    consume_request_body(forwarded).await;
    consume_response_body(harness.handler.capture_response(test_response(200, "ok"))).await;

    let capture = harness.next_capture().await;
    assert_eq!(
        capture.summary().request.mapped_uri(),
        Some("http://b.test.com/interested/orders?client=fluxcope")
    );
    harness.finish().await;
}

#[tokio::test]
async fn unmatched_prefilter_request_still_uses_map_local() {
    let path = temp_file_path("filtered.json");
    fs::write(&path, br#"{"mapped":true}"#).expect("test file should be written");
    let mut harness = Harness::new(
        prefilter_store(
            vec![],
            vec![local_rule(
                "https://a.com/some/api",
                &path.to_string_lossy(),
            )],
            vec!["https://wanted.example.com/*"],
        ),
        RecordingState::default(),
    );

    let response = match harness
        .handler
        .capture_request(request(
            "POST",
            "https://a.com/some/api?x=1",
            Body::from("ignored by map local"),
        ))
        .await
    {
        RequestOrResponse::Request(_) => panic!("map local should return a response"),
        RequestOrResponse::Response(response) => response,
    };

    assert_eq!(
        crate::capture::body_bytes(response.into_body())
            .await
            .expect("mapped response should stream")
            .as_ref(),
        br#"{"mapped":true}"#
    );
    assert!(harness.captures.try_recv().is_err());
    harness.finish().await;
}

#[tokio::test]
async fn local_mapping_streams_file_and_captures_exact_bytes() {
    let path = temp_file_path("api.json");
    fs::write(&path, br#"{"ok":true}"#).expect("test file should be written");
    let mut harness = Harness::new(
        request_policy_store(
            vec![],
            vec![local_rule(
                "https://a.com/some/api",
                &path.to_string_lossy(),
            )],
        ),
        RecordingState::default(),
    );
    let request = request("GET", "https://a.com/some/api?x=1", Body::empty());
    let response = match harness.handler.capture_request(request).await {
        RequestOrResponse::Request(_) => panic!("local mapping should return response"),
        RequestOrResponse::Response(response) => response,
    };
    assert_eq!(response.status(), StatusCode::OK);
    let body = crate::capture::body_bytes(response.into_body())
        .await
        .expect("mapped response should stream");
    assert_eq!(body.as_ref(), br#"{"ok":true}"#);

    let capture = harness.next_capture().await;
    let summary = capture.summary();
    assert_eq!(summary.response.as_ref().map(|res| res.status), Some(200));
    assert_eq!(
        summary.request.local_path.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    assert_eq!(
        capture.body_preview(BodySide::Response).flatten(),
        br#"{"ok":true}"#
    );
    assert_eq!(summary.response_body.stream, BodyStreamState::Complete);
    harness.finish().await;
}

#[tokio::test]
async fn missing_local_file_returns_and_captures_bad_gateway() {
    let path = temp_file_path("missing.json");
    let mut harness = Harness::new(
        request_policy_store(
            vec![],
            vec![local_rule("https://a.com/missing", &path.to_string_lossy())],
        ),
        RecordingState::default(),
    );
    let response = match harness
        .handler
        .capture_request(request("GET", "https://a.com/missing", Body::empty()))
        .await
    {
        RequestOrResponse::Request(_) => panic!("local mapping should return response"),
        RequestOrResponse::Response(response) => response,
    };
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let forwarded = crate::capture::body_bytes(response.into_body())
        .await
        .expect("error body should forward");
    assert!(String::from_utf8_lossy(&forwarded).contains("Failed to read mapped local file"));
    let capture = harness.next_capture().await;
    assert_eq!(
        capture.summary().response.as_ref().map(|res| res.status),
        Some(502)
    );
    let captured_error = capture.body_preview(BodySide::Response).flatten();
    assert!(String::from_utf8_lossy(&captured_error).contains("Failed to read mapped local file"));
    harness.finish().await;
}

async fn forward_request(handler: &mut LogHandler, uri: &str) -> Request<Body> {
    match handler
        .capture_request(request("GET", uri, Body::empty()))
        .await
    {
        RequestOrResponse::Request(request) => request,
        RequestOrResponse::Response(_) => panic!("test request should be forwarded"),
    }
}

async fn consume_request_body(request: Request<Body>) {
    crate::capture::body_bytes(request.into_body())
        .await
        .expect("request body should forward");
}

async fn consume_response_body(response: Response<Body>) {
    crate::capture::body_bytes(response.into_body())
        .await
        .expect("response body should forward");
}

fn request(method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .expect("test request should be valid")
}

fn test_response(status: u16, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(body.to_string()))
        .expect("test response should be valid")
}

fn request_policy_store(
    remote_rules: Vec<ProxyMapRemoteRule>,
    local_rules: Vec<ProxyMapLocalRule>,
) -> RequestPolicyStore {
    prefilter_store(remote_rules, local_rules, vec![])
}

fn prefilter_store(
    remote_rules: Vec<ProxyMapRemoteRule>,
    local_rules: Vec<ProxyMapLocalRule>,
    include_url_patterns: Vec<&str>,
) -> RequestPolicyStore {
    let proxy = ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: remote_rules,
            },
            map_local: ProxyMapLocalSettings {
                enable: true,
                rules: local_rules,
            },
        }],
    };
    let settings = AppSettings {
        proxy: Some(proxy),
        recording: crate::settings::RecordingSettings {
            prefilter: RecordingPrefilterSettings {
                enable: true,
                include_url_patterns: include_url_patterns
                    .into_iter()
                    .map(RecordingPrefilterPatternSettings::new)
                    .collect(),
            },
            ..crate::settings::RecordingSettings::default()
        },
        ..AppSettings::default()
    };
    RequestPolicyStore::new(RequestPolicy::compile(&settings).policy)
}

fn remote_rule(from: &str, to: &str) -> ProxyMapRemoteRule {
    ProxyMapRemoteRule {
        from: from.to_string(),
        to: to.to_string(),
        enable: true,
    }
}

fn local_rule(from: &str, to: &str) -> ProxyMapLocalRule {
    ProxyMapLocalRule {
        from: from.to_string(),
        to: to.to_string(),
        enable: true,
    }
}

struct TempFilePath {
    _directory: TempDir,
    path: PathBuf,
}

impl Deref for TempFilePath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for TempFilePath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

fn temp_file_path(name: &str) -> TempFilePath {
    let directory = tempfile::tempdir().expect("temporary fixture directory should be created");
    let path = directory.path().join(name);
    TempFilePath {
        _directory: directory,
        path,
    }
}
