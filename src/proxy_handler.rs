// src/proxy_handler.rs
use hudsucker::{
    async_trait::async_trait,
    hyper::{Body, Request, Response, Method},
    HttpContext, HttpHandler, RequestOrResponse,
};
use tokio::sync::mpsc;
use crate::app::AppEvent;

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
}

#[async_trait]
impl HttpHandler for LogHandler {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // In a real app, you'd clone the body stream here to capture it. 
        // For brevity, we are just capturing metadata.
        // Capturing full bodies requires `hyper::body::to_bytes` which consumes the stream,
        // so you must reconstruct the body to pass it along.
        
        let data = CapturedData {
            id: uuid::Uuid::new_v4(),
            method: req.method().clone(),
            uri: req.uri().to_string(),
            status: None,
            req_headers: req.headers().iter().map(|(k,v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect(),
            res_headers: vec![],
            req_body: None, // Simplified for this example
            res_body: None,
        };

        let _ = self.tx.send(AppEvent::NetworkRequest(data));
        RequestOrResponse::Request(req)
    }

    async fn handle_response(&mut self, _ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
        // Logic to match response to request would go here using the context
        res
    }
}
