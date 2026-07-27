use std::path::Path;

use hudsucker::{
    HttpContext, HttpHandler, RequestOrResponse,
    async_trait::async_trait,
    hyper::{
        Body, Method, Request, Response, StatusCode,
        body::Bytes,
        header::{CONTENT_LENGTH, CONTENT_TYPE},
    },
};
use tokio::{fs::File, io::AsyncReadExt};

use crate::{
    capture::{
        BodySide, BodyTaskTracker, CaptureHandle, CapturePublisher, RequestCaptureInput,
        ResponseCaptureInput, drain_body, tee_body,
    },
    recording::RecordingState,
    request_policy::RequestPolicyStore,
};

pub struct LogHandler {
    publisher: CapturePublisher,
    body_tasks: BodyTaskTracker,
    request_policy_store: RequestPolicyStore,
    recording: RecordingState,
    current_request: Option<CaptureHandle>,
}

impl LogHandler {
    pub(crate) fn new(
        publisher: CapturePublisher,
        body_tasks: BodyTaskTracker,
        request_policy_store: RequestPolicyStore,
        recording: RecordingState,
    ) -> Self {
        Self {
            publisher,
            body_tasks,
            request_policy_store,
            recording,
            current_request: None,
        }
    }

    async fn capture_request(&mut self, req: Request<Body>) -> RequestOrResponse {
        // CONNECT establishes the tunnel; the HTTP requests within it are captured separately.
        if req.method() == Method::CONNECT {
            return RequestOrResponse::Request(req);
        }

        let (mut parts, body) = req.into_parts();
        let policy = self.request_policy_store.current();
        let recording = self.recording.is_enabled();
        let (mut decision, urls) = policy.evaluate(&parts.uri, recording).into_parts();
        let recording_evaluation = urls.recording();
        if let Some(recording) = recording_evaluation.filter(|recording| !recording.included) {
            log::info!(
                "request not recorded by URL prefilter: {} {}",
                parts.method,
                recording.effective_url
            );
        }

        if let Some(mapped_uri) = decision.mapped_uri.take() {
            log::info!(
                "map remote: {} -> {}",
                urls.original_url()
                    .expect("mapped requests retain their original URL"),
                mapped_uri
            );
            parts.uri = mapped_uri;
        }
        let capture = recording_evaluation
            .filter(|recording| recording.included)
            .and_then(|recording| {
                let local_path = decision
                    .local_path
                    .as_ref()
                    .map(|path| path.display().to_string());
                self.publisher.try_start(RequestCaptureInput {
                    method: parts.method.clone(),
                    original_uri: recording.original_url,
                    effective_uri: recording.effective_url,
                    local_path: local_path.as_deref(),
                    headers: &parts.headers,
                })
            });

        if let Some(local_path) = decision.local_path {
            log::info!(
                "map local: {} -> {}",
                urls.original_url()
                    .expect("mapped requests retain their original URL"),
                local_path.display()
            );
            if let Some(capture) = capture.as_ref() {
                drain_body(body, capture.clone(), &self.body_tasks);
            } else {
                discard_body(body, &self.body_tasks);
            }
            self.current_request = capture;
            let response = local_file_response(&local_path, &self.body_tasks).await;
            return RequestOrResponse::Response(self.capture_response(response));
        }

        let body = match capture.as_ref() {
            Some(capture) => tee_body(body, capture.clone(), BodySide::Request, &self.body_tasks),
            None => body,
        };
        self.current_request = capture;
        RequestOrResponse::Request(Request::from_parts(parts, body))
    }

    fn capture_response(&mut self, res: Response<Body>) -> Response<Body> {
        let Some(capture) = self.current_request.take() else {
            return res;
        };
        let (parts, body) = res.into_parts();
        capture.set_response(ResponseCaptureInput {
            status: parts.status.as_u16(),
            headers: &parts.headers,
        });
        let body = tee_body(body, capture, BodySide::Response, &self.body_tasks);
        Response::from_parts(parts, body)
    }
}

impl Clone for LogHandler {
    fn clone(&self) -> Self {
        Self {
            publisher: self.publisher.clone(),
            body_tasks: self.body_tasks.clone(),
            request_policy_store: self.request_policy_store.clone(),
            recording: self.recording.clone(),
            current_request: None,
        }
    }
}

#[async_trait]
impl HttpHandler for LogHandler {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        self.capture_request(req).await
    }

    async fn handle_response(&mut self, _ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
        self.capture_response(res)
    }

    async fn handle_error(
        &mut self,
        _ctx: &HttpContext,
        error: hudsucker::hyper::Error,
    ) -> Response<Body> {
        log::error!("Failed to forward request: {error}");
        if let Some(capture) = self.current_request.take() {
            let headers = hudsucker::hyper::HeaderMap::new();
            capture.set_response(ResponseCaptureInput {
                status: StatusCode::BAD_GATEWAY.as_u16(),
                headers: &headers,
            });
            capture.fail(
                BodySide::Response,
                format!("upstream request failed: {error}"),
            );
        }
        Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::empty())
            .unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

fn discard_body(mut source: Body, tasks: &BodyTaskTracker) {
    use hudsucker::hyper::body::HttpBody as _;

    let shutdown = tasks.shutdown_token();
    tasks.spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                next = source.data() => match next {
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        log::debug!("discarded mapped request body failed: {error}");
                        return;
                    }
                    None => return,
                }
            }
        }
    });
}

async fn local_file_response(path: &Path, tasks: &BodyTaskTracker) -> Response<Body> {
    match File::open(path).await {
        Ok(file) => {
            let length = match file.metadata().await {
                Ok(metadata) => metadata.len(),
                Err(error) => return local_file_error_response(path, error),
            };
            let (sender, body) = Body::channel();
            let shutdown = tasks.shutdown_token();
            tasks.spawn(stream_local_file(file, sender, shutdown));
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, content_type_for_path(path))
                .header(CONTENT_LENGTH, length.to_string())
                .body(body)
                .unwrap_or_else(|_| Response::new(Body::empty()))
        }
        Err(error) => local_file_error_response(path, error),
    }
}

async fn stream_local_file(
    mut file: File,
    mut sender: hudsucker::hyper::body::Sender,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = tokio::select! {
            _ = shutdown.cancelled() => {
                sender.abort();
                return;
            }
            read = file.read(&mut buffer) => read,
        };
        match read {
            Ok(0) => return,
            Ok(read) => {
                let sent = tokio::select! {
                    _ = shutdown.cancelled() => {
                        sender.abort();
                        return;
                    }
                    sent = sender.send_data(Bytes::copy_from_slice(&buffer[..read])) => sent,
                };
                if sent.is_err() {
                    return;
                }
            }
            Err(error) => {
                log::error!("Failed to stream mapped local file: {error}");
                sender.abort();
                return;
            }
        }
    }
}

fn local_file_error_response(path: &Path, error: std::io::Error) -> Response<Body> {
    let body = format!(
        "Failed to read mapped local file {}: {error}",
        path.display()
    );
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, body.len().to_string())
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => "application/json",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript",
        Some("txt") | Some("text") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::{
        capture::{BodyStreamState, CapturePolicy, CaptureRecord, CaptureSequence},
        request_policy::RequestPolicy,
        settings::{
            AppSettings, ProxyMapLocalRule, ProxyMapLocalSettings, ProxyMapRemoteRule,
            ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
            RecordingPrefilterPatternSettings, RecordingPrefilterSettings,
        },
    };
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
            let handler =
                LogHandler::new(publisher, tasks.clone(), request_policy_store, recording);
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
            slow.body_preview(BodySide::Response).as_ref().as_ref(),
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
            fast.body_preview(BodySide::Response).as_ref().as_ref(),
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
        let (mut source, body) = Body::channel();
        let mut response = harness.handler.capture_response(Response::new(body));
        source
            .send_data(Bytes::from_static(b"first"))
            .await
            .expect("source should accept first chunk");

        use hudsucker::hyper::body::HttpBody as _;
        let first = tokio::time::timeout(Duration::from_secs(1), response.body_mut().data())
            .await
            .expect("forwarded chunk must not wait for EOF")
            .expect("response should contain a chunk")
            .expect("chunk should be forwarded");
        assert_eq!(first.as_ref(), b"first");
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
            hudsucker::hyper::body::to_bytes(forwarded.into_body())
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
            hudsucker::hyper::body::to_bytes(forwarded.into_body())
                .await
                .expect("request body should forward")
                .as_ref(),
            b"request payload"
        );
        let response = harness
            .handler
            .capture_response(test_response(200, "response payload"));
        assert_eq!(
            hudsucker::hyper::body::to_bytes(response.into_body())
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
            prefilter_store(vec![], vec![], vec!["*://*.xiaojukeji.com/*"]),
            RecordingState::default(),
        );
        let uri = "https://omgup.xiaojukeji.com:443/api/ministat/x?count=1&e=tech_socket_error";

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
            "https://a.com/interested/orders?client=wirelens",
            Body::empty(),
        );

        let forwarded = match harness.handler.capture_request(request).await {
            RequestOrResponse::Request(request) => request,
            RequestOrResponse::Response(_) => panic!("remote mapping should forward"),
        };
        assert_eq!(
            forwarded.uri().to_string(),
            "http://b.test.com/interested/orders?client=wirelens"
        );
        consume_request_body(forwarded).await;
        consume_response_body(harness.handler.capture_response(test_response(200, "ok"))).await;

        let capture = harness.next_capture().await;
        assert_eq!(
            capture.summary().request.mapped_uri(),
            Some("http://b.test.com/interested/orders?client=wirelens")
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
            hudsucker::hyper::body::to_bytes(response.into_body())
                .await
                .expect("mapped response should stream")
                .as_ref(),
            br#"{"mapped":true}"#
        );
        assert!(harness.captures.try_recv().is_err());
        let _ = fs::remove_file(path);
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
        let body = hudsucker::hyper::body::to_bytes(response.into_body())
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
            capture.body_preview(BodySide::Response).as_ref().as_ref(),
            br#"{"ok":true}"#
        );
        assert_eq!(summary.response_body.stream, BodyStreamState::Complete);
        let _ = fs::remove_file(path);
        harness.finish().await;
    }

    #[tokio::test]
    async fn missing_local_file_returns_and_captures_bad_gateway() {
        let path = temp_file_path("missing.json");
        let _ = fs::remove_file(&path);
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
        let forwarded = hudsucker::hyper::body::to_bytes(response.into_body())
            .await
            .expect("error body should forward");
        assert!(String::from_utf8_lossy(&forwarded).contains("Failed to read mapped local file"));
        let capture = harness.next_capture().await;
        assert_eq!(
            capture.summary().response.as_ref().map(|res| res.status),
            Some(502)
        );
        assert!(
            String::from_utf8_lossy(&capture.body_preview(BodySide::Response))
                .contains("Failed to read mapped local file")
        );
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
        hudsucker::hyper::body::to_bytes(request.into_body())
            .await
            .expect("request body should forward");
    }

    async fn consume_response_body(response: Response<Body>) {
        hudsucker::hyper::body::to_bytes(response.into_body())
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

    fn temp_file_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wirelens-{nanos}-{name}"))
    }
}
