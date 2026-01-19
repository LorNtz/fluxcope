// src/proxy_handler.rs
use crate::app::AppEvent;
use hudsucker::{
    HttpContext, HttpHandler, RequestOrResponse,
    async_trait::async_trait,
    hyper::{Body, Method, Request, Response},
};
use std::collections::HashMap;
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
            Ok(bytes) => {
                // Try UTF-8 first, if that fails try to detect encoding or show hex representation
                let body_str = if let Ok(s) = String::from_utf8(bytes.clone().to_vec()) {
                    Some(s)
                } else {
                    // For non-UTF8 bodies (like gzipped or binary data), show hex representation
                    if bytes.is_empty() {
                        None
                    } else {
                        Some(format!("[Binary body: {} bytes, first 32 bytes in hex: {}]",
                            bytes.len(),
                            bytes.iter().take(32).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")))
                    }
                };
                (body_str, bytes)
            }
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
            Ok(bytes) => {
                // Try UTF-8 first, if that fails try to detect encoding or show hex representation
                let body_str = if let Ok(s) = String::from_utf8(bytes.clone().to_vec()) {
                    Some(s)
                } else {
                    // For non-UTF8 bodies (like gzipped or binary data), show hex representation
                    if bytes.is_empty() {
                        None
                    } else {
                        Some(format!("[Binary body: {} bytes, first 32 bytes in hex: {}]",
                            bytes.len(),
                            bytes.iter().take(32).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")))
                    }
                };
                (body_str, bytes)
            }
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
