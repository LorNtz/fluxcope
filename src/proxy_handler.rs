use std::path::Path;

use hudsucker::{
    Body, HttpContext, HttpHandler, RequestOrResponse,
    hyper::{
        Method, Request, Response, StatusCode,
        body::Bytes,
        header::{CONTENT_LENGTH, CONTENT_TYPE},
    },
};
use tokio::{fs::File, io::AsyncReadExt};

use crate::{
    capture::{
        BodySender, BodySide, BodyTaskTracker, CaptureHandle, CapturePublisher,
        RequestCaptureInput, ResponseCaptureInput, body_channel, drain_body, tee_body,
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
        error: hyper_util::client::legacy::Error,
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
    use http_body_util::BodyExt as _;

    let shutdown = tasks.shutdown_token();
    tasks.spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                next = source.frame() => match next {
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
            let (sender, body) = body_channel();
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
    mut sender: BodySender,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = tokio::select! {
            _ = shutdown.cancelled() => {
                sender.abort(std::io::Error::new(std::io::ErrorKind::Interrupted, "body stream interrupted").into());
                return;
            }
            read = file.read(&mut buffer) => read,
        };
        match read {
            Ok(0) => return,
            Ok(read) => {
                let sent = tokio::select! {
                    _ = shutdown.cancelled() => {
                        sender.abort(std::io::Error::new(std::io::ErrorKind::Interrupted, "body stream interrupted").into());
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
                sender.abort(
                    std::io::Error::new(std::io::ErrorKind::Interrupted, "body stream interrupted")
                        .into(),
                );
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
mod tests;
