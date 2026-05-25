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
use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CapturedData {
    pub id: uuid::Uuid,
    pub method: Method,
    pub uri: String,
    pub status: Option<u16>,
    pub req_headers: Vec<(String, String)>,
    pub res_headers: Vec<(String, String)>,
    pub req_body: Option<String>,
    pub res_body: Option<String>,
}

#[derive(Clone)]
pub struct LogHandler {
    pub tx: mpsc::UnboundedSender<AppEvent>,
    pub pending_requests: Arc<Mutex<HashMap<uuid::Uuid, CapturedData>>>,
}

#[async_trait]
impl HttpHandler for LogHandler {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // Skip CONNECT requests - they're just for establishing HTTPS tunnels
        // and not actual application requests we want to display
        if req.method() == Method::CONNECT {
            return RequestOrResponse::Request(req);
        }

        let id = uuid::Uuid::new_v4();
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

        let mut pending = self.pending_requests.lock().await;
        pending.insert(id, data);
        drop(pending);

        RequestOrResponse::Request(reconstructed_req)
    }

    async fn handle_response(&mut self, _ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
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

        let mut pending = self.pending_requests.lock().await;
        if let Some(mut data) = pending.values().next().cloned() {
            data.status = Some(status);
            data.res_headers = res_headers;
            data.res_body = res_body;
            let _ = self.tx.send(AppEvent::NetworkRequest(data));
            pending.clear();
        }
        drop(pending);

        reconstructed_res
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
}
