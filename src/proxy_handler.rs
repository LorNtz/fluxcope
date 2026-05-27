// src/proxy_handler.rs
use crate::app::AppEvent;
use hudsucker::{
    HttpContext, HttpHandler, RequestOrResponse,
    async_trait::async_trait,
    hyper::{
        Body, Method, Request, Response,
        header::{CONTENT_ENCODING, HeaderMap},
    },
};
use std::io::{self, Read};
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
    pub status: Option<u16>,
    pub req_headers: Vec<(String, String)>,
    pub res_headers: Vec<(String, String)>,
    pub req_body: Option<String>,
    pub res_body: Option<String>,
}

pub struct LogHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    next_sequence: Arc<AtomicU64>,
    current_request: Option<CapturedData>,
}

impl LogHandler {
    pub fn new(tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self {
            tx,
            next_sequence: Arc::new(AtomicU64::new(0)),
            current_request: None,
        }
    }

    async fn capture_request(&mut self, req: Request<Body>) -> RequestOrResponse {
        // Skip CONNECT requests - they're just for establishing HTTPS tunnels
        // and not actual application requests we want to display
        if req.method() == Method::CONNECT {
            return RequestOrResponse::Request(req);
        }

        let id = uuid::Uuid::new_v4();
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let (parts, body) = req.into_parts();

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

        let reconstructed_req = Request::from_parts(parts, Body::from(req_body_bytes));

        self.current_request = Some(data);

        RequestOrResponse::Request(reconstructed_req)
    }

    async fn capture_response(&mut self, res: Response<Body>) -> Response<Body> {
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

        if let Some(mut data) = self.current_request.take() {
            data.status = Some(status);
            data.res_headers = res_headers;
            data.res_body = res_body;
            let _ = self.tx.send(AppEvent::NetworkRequest(data));
        }

        reconstructed_res
    }
}

impl Clone for LogHandler {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            next_sequence: Arc::clone(&self.next_sequence),
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

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use hudsucker::hyper::header::HeaderValue;
    use std::io::Write;

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
        let base_handler = LogHandler::new(tx);
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
}
