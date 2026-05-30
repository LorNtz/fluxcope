// src/proxy_handler.rs
use crate::{app::AppEvent, mapping::MappingStore, recording::RecordingState};
use hudsucker::{
    HttpContext, HttpHandler, RequestOrResponse,
    async_trait::async_trait,
    hyper::{
        Body, Method, Request, Response, StatusCode,
        header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap},
    },
};
use std::io::{self, Read};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CapturedData {
    pub id: uuid::Uuid,
    pub sequence: u64,
    pub method: Method,
    pub uri: String,
    pub mapped_uri: Option<String>,
    pub local_path: Option<String>,
    pub status: Option<u16>,
    pub req_headers: Vec<(String, String)>,
    pub res_headers: Vec<(String, String)>,
    pub req_body: Option<String>,
    pub res_body: Option<String>,
}

pub struct LogHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    next_sequence: Arc<AtomicU64>,
    mapping_store: MappingStore,
    recording: RecordingState,
    current_request: Option<CapturedData>,
}

impl LogHandler {
    pub fn new(
        tx: mpsc::UnboundedSender<AppEvent>,
        mapping_store: MappingStore,
        recording: RecordingState,
    ) -> Self {
        Self {
            tx,
            next_sequence: Arc::new(AtomicU64::new(0)),
            mapping_store,
            recording,
            current_request: None,
        }
    }

    async fn capture_request(&mut self, req: Request<Body>) -> RequestOrResponse {
        // Skip CONNECT requests - they're just for establishing HTTPS tunnels
        // and not actual application requests we want to display
        if req.method() == Method::CONNECT {
            return RequestOrResponse::Request(req);
        }

        if !self.recording.is_enabled() {
            return self.forward_uncaptured_request(req).await;
        }

        let id = uuid::Uuid::new_v4();
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let (mut parts, body) = req.into_parts();

        let (req_body, req_body_bytes) = match hyper::body::to_bytes(body).await {
            Ok(bytes) => (body_for_display(&bytes, &parts.headers), bytes),
            Err(e) => {
                log::error!("Failed to read request body: {}", e);
                (None, hyper::body::Bytes::new())
            }
        };

        let data = CapturedData {
            id,
            sequence,
            method: parts.method.clone(),
            uri: parts.uri.to_string(),
            mapped_uri: None,
            local_path: None,
            status: None,
            req_headers: parts
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect(),
            res_headers: vec![],
            req_body,
            res_body: None,
        };

        let mapping = self.mapping_store.current();
        let decision = mapping.map_request(&parts.uri);
        let original_uri = data.uri.clone();
        let mut data = data;

        if let Some(mapped_uri) = decision.mapped_uri {
            log::info!("map remote: {} -> {}", original_uri, mapped_uri);
            data.mapped_uri = Some(mapped_uri.to_string());
            parts.uri = mapped_uri;
        }

        let reconstructed_req = Request::from_parts(parts, Body::from(req_body_bytes));

        self.current_request = Some(data);

        if let Some(local_path) = decision.local_path {
            if let Some(data) = &mut self.current_request {
                data.local_path = Some(local_path.display().to_string());
            }
            log::info!("map local: {} -> {}", original_uri, local_path.display());
            let response = local_file_response(&local_path).await;
            return RequestOrResponse::Response(self.capture_response(response).await);
        }

        RequestOrResponse::Request(reconstructed_req)
    }

    async fn forward_uncaptured_request(&self, req: Request<Body>) -> RequestOrResponse {
        let (mut parts, body) = req.into_parts();
        let original_uri = parts.uri.to_string();
        let mapping = self.mapping_store.current();
        let decision = mapping.map_request(&parts.uri);

        if let Some(mapped_uri) = decision.mapped_uri {
            log::info!("map remote: {} -> {}", original_uri, mapped_uri);
            parts.uri = mapped_uri;
        }

        if let Some(local_path) = decision.local_path {
            log::info!("map local: {} -> {}", original_uri, local_path.display());
            return RequestOrResponse::Response(local_file_response(&local_path).await);
        }

        RequestOrResponse::Request(Request::from_parts(parts, body))
    }

    async fn capture_response(&mut self, res: Response<Body>) -> Response<Body> {
        let Some(mut data) = self.current_request.take() else {
            return res;
        };
        let (parts, body) = res.into_parts();

        let (res_body, res_body_bytes) = match hyper::body::to_bytes(body).await {
            Ok(bytes) => (body_for_display(&bytes, &parts.headers), bytes),
            Err(e) => {
                log::error!("Failed to read response body: {}", e);
                (None, hyper::body::Bytes::new())
            }
        };

        let status = parts.status.as_u16();
        let res_headers: Vec<(String, String)> = parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let reconstructed_res = Response::from_parts(parts, Body::from(res_body_bytes));

        data.status = Some(status);
        data.res_headers = res_headers;
        data.res_body = res_body;
        let _ = self.tx.send(AppEvent::NetworkRequest(data));

        reconstructed_res
    }
}

impl Clone for LogHandler {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            next_sequence: Arc::clone(&self.next_sequence),
            mapping_store: self.mapping_store.clone(),
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
        self.capture_response(res).await
    }
}

fn body_for_display(bytes: &[u8], headers: &HeaderMap) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }

    let display_bytes = match decode_content_encoded_body(bytes, headers) {
        Ok(display_bytes) => display_bytes,
        Err(error) => {
            log::warn!("Failed to decode body for display: {error}");
            bytes.to_vec()
        }
    };

    String::from_utf8(display_bytes).ok().or_else(|| {
        Some(format!(
            "[Binary body: {} bytes, first 32 bytes in hex: {}]",
            bytes.len(),
            bytes
                .iter()
                .take(32)
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ")
        ))
    })
}

fn decode_content_encoded_body(bytes: &[u8], headers: &HeaderMap) -> io::Result<Vec<u8>> {
    let encodings = content_encodings(headers);
    if encodings.is_empty() {
        return Ok(bytes.to_vec());
    }

    let mut decoded = bytes.to_vec();
    for encoding in encodings.iter().rev() {
        decoded = match encoding.as_str() {
            "gzip" | "x-gzip" => read_all(flate2::read::GzDecoder::new(decoded.as_slice()))?,
            "deflate" => decode_deflate(&decoded)?,
            "br" => read_all(brotli::Decompressor::new(decoded.as_slice(), 4096))?,
            "zstd" => read_all(zstd::stream::read::Decoder::new(decoded.as_slice())?)?,
            "identity" => decoded,
            _ => return Ok(decoded),
        };
    }

    Ok(decoded)
}

fn content_encodings(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|encoding| encoding.trim().to_ascii_lowercase())
        .filter(|encoding| !encoding.is_empty() && encoding != "identity")
        .collect()
}

fn decode_deflate(bytes: &[u8]) -> io::Result<Vec<u8>> {
    read_all(flate2::read::ZlibDecoder::new(bytes))
        .or_else(|_| read_all(flate2::read::DeflateDecoder::new(bytes)))
}

fn read_all(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded)?;
    Ok(decoded)
}

async fn local_file_response(path: &Path) -> Response<Body> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, content_type_for_path(path))
            .header(CONTENT_LENGTH, bytes.len().to_string())
            .body(Body::from(bytes))
            .unwrap_or_else(|_| Response::new(Body::empty())),
        Err(error) => {
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
    }
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
    use super::*;
    use crate::{
        mapping::MappingEngine,
        recording::RecordingState,
        settings::{
            ProxyMapLocalRule, ProxyMapLocalSettings, ProxyMapRemoteRule, ProxyMapRemoteSettings,
            ProxyPresetSettings, ProxySettings,
        },
    };
    use flate2::{Compression, write::GzEncoder};
    use hudsucker::hyper::header::HeaderValue;
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn decodes_gzip_body_for_display() {
        let body = "compressed response text";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(body.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        assert_eq!(
            Some(body.to_string()),
            body_for_display(&compressed, &headers)
        );
    }

    #[test]
    fn still_summarizes_non_utf8_body() {
        let headers = HeaderMap::new();
        let body = [0xff, 0x00, 0x80];

        assert_eq!(
            Some("[Binary body: 3 bytes, first 32 bytes in hex: ff 00 80]".to_string()),
            body_for_display(&body, &headers)
        );
    }

    #[tokio::test]
    async fn cloned_handlers_capture_concurrent_requests_independently() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let base_handler = LogHandler::new(tx, MappingStore::default(), RecordingState::default());
        let mut slow_handler = base_handler.clone();
        let mut fast_handler = base_handler.clone();

        forward_request(&mut slow_handler, "https://example.com/slow").await;
        forward_request(&mut fast_handler, "https://example.com/fast").await;

        fast_handler
            .capture_response(test_response(200, "fast body"))
            .await;
        slow_handler
            .capture_response(test_response(201, "slow body"))
            .await;

        let fast = received_request(&mut rx);
        let slow = received_request(&mut rx);

        assert_eq!(fast.sequence, 1);
        assert_eq!(fast.uri, "https://example.com/fast");
        assert_eq!(fast.status, Some(200));
        assert_eq!(fast.res_body.as_deref(), Some("fast body"));
        assert_eq!(slow.sequence, 0);
        assert_eq!(slow.uri, "https://example.com/slow");
        assert_eq!(slow.status, Some(201));
        assert_eq!(slow.res_body.as_deref(), Some("slow body"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn remote_mapping_rewrites_forwarded_uri_but_captures_original_uri() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut handler = LogHandler::new(
            tx,
            mapping_store(
                vec![remote_rule("https://a.com", "http://b.test.com")],
                vec![],
            ),
            RecordingState::default(),
        );
        let request = Request::builder()
            .method(Method::GET)
            .uri("https://a.com/some/api?x=1")
            .body(Body::empty())
            .expect("test request should be valid");

        let forwarded = match handler.capture_request(request).await {
            RequestOrResponse::Request(request) => request,
            RequestOrResponse::Response(_) => panic!("remote mapping should forward request"),
        };

        assert_eq!(
            forwarded.uri().to_string(),
            "http://b.test.com/some/api?x=1"
        );

        handler.capture_response(test_response(200, "ok")).await;

        let captured = received_request(&mut rx);
        assert_eq!(captured.uri, "https://a.com/some/api?x=1");
        assert_eq!(
            captured.mapped_uri.as_deref(),
            Some("http://b.test.com/some/api?x=1")
        );
        assert_eq!(captured.local_path, None);
        assert_eq!(captured.status, Some(200));
    }

    #[tokio::test]
    async fn recording_off_forwards_mapped_request_without_capture() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut handler = LogHandler::new(
            tx,
            mapping_store(
                vec![remote_rule("https://a.com", "http://b.test.com")],
                vec![],
            ),
            RecordingState::new(false),
        );
        let request = Request::builder()
            .method(Method::POST)
            .uri("https://a.com/some/api?x=1")
            .header("x-keep", "1")
            .body(Body::from("payload"))
            .expect("test request should be valid");

        let forwarded = match handler.capture_request(request).await {
            RequestOrResponse::Request(request) => request,
            RequestOrResponse::Response(_) => panic!("remote mapping should forward request"),
        };

        assert_eq!(
            forwarded.uri().to_string(),
            "http://b.test.com/some/api?x=1"
        );
        assert_eq!(forwarded.headers()["x-keep"], "1");
        let body = hyper::body::to_bytes(forwarded.into_body())
            .await
            .expect("forwarded body should read");
        assert_eq!(body.as_ref(), b"payload");

        handler.capture_response(test_response(200, "ok")).await;

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn recording_off_returns_local_mapping_response_without_capture() {
        let path = temp_file_path("api-recording-off.json");
        fs::write(&path, r#"{"mock":true}"#).expect("test file should be written");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut handler = LogHandler::new(
            tx,
            mapping_store(
                vec![],
                vec![local_rule(
                    "https://a.com/some/api1",
                    &path.to_string_lossy(),
                )],
            ),
            RecordingState::new(false),
        );
        let request = Request::builder()
            .method(Method::GET)
            .uri("https://a.com/some/api1?x=1")
            .body(Body::empty())
            .expect("test request should be valid");

        let response = match handler.capture_request(request).await {
            RequestOrResponse::Request(_) => panic!("local mapping should return response"),
            RequestOrResponse::Response(response) => response,
        };

        assert_eq!(response.status(), StatusCode::OK);
        let body = hyper::body::to_bytes(response.into_body())
            .await
            .expect("test response body should read");
        assert_eq!(body.as_ref(), br#"{"mock":true}"#);
        assert!(rx.try_recv().is_err());

        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn local_mapping_returns_file_response_and_records_capture() {
        let path = temp_file_path("api1.json");
        fs::write(&path, r#"{"ok":true}"#).expect("test file should be written");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut handler = LogHandler::new(
            tx,
            mapping_store(
                vec![],
                vec![local_rule(
                    "https://a.com/some/api1",
                    &path.to_string_lossy(),
                )],
            ),
            RecordingState::default(),
        );
        let request = Request::builder()
            .method(Method::GET)
            .uri("https://a.com/some/api1?x=1")
            .body(Body::empty())
            .expect("test request should be valid");

        let response = match handler.capture_request(request).await {
            RequestOrResponse::Request(_) => panic!("local mapping should return response"),
            RequestOrResponse::Response(response) => response,
        };

        assert_eq!(response.status(), StatusCode::OK);
        let body = hyper::body::to_bytes(response.into_body())
            .await
            .expect("test response body should read");
        assert_eq!(body.as_ref(), br#"{"ok":true}"#);

        let captured = received_request(&mut rx);
        assert_eq!(captured.uri, "https://a.com/some/api1?x=1");
        assert_eq!(captured.status, Some(200));
        assert_eq!(captured.res_body.as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(
            captured.local_path.as_deref(),
            Some(path.to_string_lossy().as_ref())
        );

        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn missing_local_file_returns_bad_gateway_and_records_error_body() {
        let path = temp_file_path("missing.json");
        let _ = fs::remove_file(&path);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut handler = LogHandler::new(
            tx,
            mapping_store(
                vec![],
                vec![local_rule("https://a.com/missing", &path.to_string_lossy())],
            ),
            RecordingState::default(),
        );
        let request = Request::builder()
            .method(Method::GET)
            .uri("https://a.com/missing")
            .body(Body::empty())
            .expect("test request should be valid");

        let response = match handler.capture_request(request).await {
            RequestOrResponse::Request(_) => panic!("local mapping should return response"),
            RequestOrResponse::Response(response) => response,
        };

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let captured = received_request(&mut rx);
        assert_eq!(captured.status, Some(502));
        assert!(
            captured
                .res_body
                .as_deref()
                .is_some_and(|body| body.contains("Failed to read mapped local file"))
        );
    }

    async fn forward_request(handler: &mut LogHandler, uri: &str) {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .expect("test request should be valid");

        match handler.capture_request(request).await {
            RequestOrResponse::Request(_) => {}
            RequestOrResponse::Response(_) => panic!("test request should be forwarded"),
        }
    }

    fn test_response(status: u16, body: &str) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Body::from(body.to_string()))
            .expect("test response should be valid")
    }

    fn received_request(rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> CapturedData {
        match rx.try_recv().expect("request should have been captured") {
            AppEvent::NetworkRequest(data) => data,
            AppEvent::LogMessage(_) | AppEvent::CertificateDownloadReady(_) => {
                panic!("unexpected app event")
            }
        }
    }

    fn mapping_store(
        remote_rules: Vec<ProxyMapRemoteRule>,
        local_rules: Vec<ProxyMapLocalRule>,
    ) -> MappingStore {
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

        MappingStore::new(MappingEngine::compile(Some(&proxy)))
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
