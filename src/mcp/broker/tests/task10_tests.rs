use super::*;
use crate::{
    capture::{
        BodySide, BodyStreamState, CaptureRecord, CaptureSequence, CaptureSnapshotMode,
        CapturedExchange,
    },
    control::{
        body::{BodyPage, BodyPageSource, BodyRange, BodyRepresentation},
        capture_query::CaptureDetail,
    },
    control_rpc::protocol::{ControlOperation, ControlResult, InstanceScope},
};
use hyper::{Method, body::Bytes};
use rmcp::{
    ServiceError,
    model::{CallToolRequestParams, ReadResourceRequestParams},
};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().expect("tool arguments").clone()
}

fn tool_json(result: &rmcp::model::CallToolResult) -> Value {
    if let Some(value) = &result.structured_content {
        return value.clone();
    }
    let value = serde_json::to_value(result).expect("tool result JSON");
    let text = value["content"][0]["text"]
        .as_str()
        .expect("tool JSON text");
    serde_json::from_str(text).expect("tool structured JSON")
}

fn capture_detail(response: bool) -> CaptureDetail {
    let record = CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(0),
        method: Method::POST,
        uri: "https://example.test/body".to_owned(),
        mapped_uri: None,
        local_path: None,
        status: response.then_some(200),
        req_headers: vec![("content-type".to_owned(), "text/plain".to_owned())],
        res_headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        req_body: Some(String::new()),
        res_body: response.then(|| "response".to_owned()),
    });
    CaptureDetail::from_snapshot(&record.snapshot(CaptureSnapshotMode::MetadataOnly))
}

fn body_page(request: &crate::control::body::BodyContentRequest) -> BodyPage {
    BodyPage {
        content: match request.representation {
            BodyRepresentation::Raw => Bytes::from_static(&[0xff, 0x00]),
            BodyRepresentation::Decoded => Bytes::from_static(b"ok"),
        },
        media_type: Some("text/plain; charset=utf-8".to_owned()),
        requested_range: BodyRange {
            offset: request.offset,
            length: request.length,
        },
        actual_range: BodyRange {
            offset: request.offset,
            length: 2,
        },
        total_bytes: 2,
        next_offset: None,
        source: BodyPageSource {
            stream: BodyStreamState::Complete,
            observed_bytes: 2,
            retained_bytes: 2,
            truncated: false,
            truncation_reason: None,
            decoded_encoding_chain: vec![],
            decoded_output_limited: false,
        },
    }
}

#[derive(Clone)]
struct BodyProbe {
    descriptor: InstanceDescriptor,
    calls: Arc<Mutex<Vec<ControlOperation>>>,
    detail: CaptureDetail,
    body_error: Option<ControlError>,
}

impl InstanceProbe for BodyProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move { Ok(describe(descriptor, 1)) })
    }

    fn call<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        operation: ControlOperation,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("body calls")
                .push(operation.clone());
            match operation {
                ControlOperation::GetCapture { .. } => Ok(ControlResult::GetCapture {
                    instance: InstanceScope {
                        proxy_endpoint: descriptor.proxy_endpoint(),
                        run_id: descriptor.run_id().clone(),
                    },
                    capture: Box::new(self.detail.clone()),
                }),
                ControlOperation::ReadCaptureBody(request) => {
                    if let Some(error) = &self.body_error {
                        return Err(error.clone());
                    }
                    Ok(ControlResult::ReadCaptureBody {
                        instance: InstanceScope {
                            proxy_endpoint: descriptor.proxy_endpoint(),
                            run_id: descriptor.run_id().clone(),
                        },
                        page: Box::new(body_page(&request)),
                    })
                }
                other => panic!("unexpected operation: {other:?}"),
            }
        })
    }
}

fn body_broker(probe: Arc<BodyProbe>) -> Broker {
    Broker::with_dependencies(
        PathBuf::from("/test/.wirelens/run/instances"),
        FakeRegistry::new(vec![probe.descriptor.clone()]),
        probe,
    )
}

async fn client_for(
    broker: Broker,
) -> (
    rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
    tokio::task::JoinHandle<()>,
) {
    let (client_transport, server_transport) = duplex(128 * 1_024);
    let server = tokio::spawn(async move {
        let service = broker.serve(server_transport).await.expect("serve broker");
        service.waiting().await.expect("broker shutdown");
    });
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("task-10-client", "1"),
    );
    let client = client_info
        .serve(client_transport)
        .await
        .expect("initialize client");
    (client, server)
}

fn selected_arguments() -> Value {
    json!({
        "instance": {
            "proxy_endpoint": "127.0.0.1:19010",
            "run_id": RUN_A
        },
        "capture_id": 0,
        "expected_revision": 0
    })
}

#[tokio::test]
async fn get_capture_returns_selected_revision_side_links_that_are_readable_consistently() {
    let descriptor = descriptor(19010, RUN_A);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let probe = Arc::new(BodyProbe {
        descriptor,
        calls: Arc::clone(&calls),
        detail: capture_detail(true),
        body_error: None,
    });
    let (client, server) = client_for(body_broker(probe)).await;
    let detail = client
        .call_tool(
            CallToolRequestParams::new("get_capture")
                .with_arguments(arguments(selected_arguments())),
        )
        .await
        .expect("get_capture");
    let value = tool_json(&detail);
    assert_eq!(
        value["instance"],
        json!({"proxy_endpoint": "127.0.0.1:19010", "run_id": RUN_A})
    );
    assert_eq!(value["capture"]["capture_sequence"], json!(0));
    assert_eq!(value["capture"]["capture_revision"], json!(0));
    let first = format!(
        "wirelens://127.0.0.1:19010/runs/{RUN_A}/captures/0/revisions/0/bodies/request/content/raw?offset=0&length=8192"
    );
    assert_eq!(value["body_resources"]["request"]["raw"], first);
    assert_eq!(
        value["body_resources"]["request"]["decoded"],
        first.replace("/raw?", "/decoded?")
    );
    assert_eq!(
        value["body_resources"]["response"]["raw"],
        first.replace("/request/", "/response/")
    );
    assert_eq!(
        value["body_resources"]["response"]["decoded"],
        first
            .replace("/request/", "/response/")
            .replace("/raw?", "/decoded?")
    );

    let resource = client
        .read_resource(ReadResourceRequestParams::new(first.clone()))
        .await
        .expect("linked resource read");
    assert_eq!(resource.contents.len(), 1);
    let content = serde_json::to_value(&resource.contents[0]).unwrap();
    assert_eq!(content["uri"], first);
    assert_eq!(content["blob"], json!("/wA="));

    {
        let operations = calls.lock().expect("body calls");
        assert!(matches!(
            operations[0],
            ControlOperation::GetCapture {
                capture_id,
                expected_revision: Some(0)
            } if capture_id == CaptureSequence::new(0)
        ));
        assert!(matches!(
            &operations[1],
            ControlOperation::ReadCaptureBody(request)
                if request.capture_id == CaptureSequence::new(0)
                    && request.capture_revision == 0
                    && request.side == BodySide::Request
                    && request.representation == BodyRepresentation::Raw
                    && request.offset == 0
                    && request.length == 8_192
        ));
    }
    client.cancel().await.expect("close client");
    server.await.expect("server task");
}

#[tokio::test]
async fn get_capture_emits_empty_request_body_links_but_no_response_links_before_metadata_exists() {
    let descriptor = descriptor(19010, RUN_A);
    let probe = Arc::new(BodyProbe {
        descriptor,
        calls: Arc::new(Mutex::new(Vec::new())),
        detail: capture_detail(false),
        body_error: None,
    });
    let (client, server) = client_for(body_broker(probe)).await;
    let result = client
        .call_tool(
            CallToolRequestParams::new("get_capture")
                .with_arguments(arguments(selected_arguments())),
        )
        .await
        .expect("get_capture");
    let value = tool_json(&result);
    assert!(value["body_resources"]["request"]["raw"].is_string());
    assert!(value["body_resources"]["request"]["decoded"].is_string());
    assert_eq!(value["body_resources"]["response"], Value::Null);
    client.cancel().await.expect("close client");
    server.await.expect("server task");
}

#[tokio::test]
async fn resource_resolution_requires_the_exact_endpoint_generation_without_retargeting() {
    let descriptor = descriptor(19010, RUN_A);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let probe = Arc::new(BodyProbe {
        descriptor,
        calls: Arc::clone(&calls),
        detail: capture_detail(true),
        body_error: None,
    });
    let (client, server) = client_for(body_broker(probe)).await;
    let stale_uri = format!(
        "wirelens://127.0.0.1:19010/runs/{RUN_B}/captures/0/revisions/0/bodies/request/content/raw?offset=0&length=8192"
    );
    let error = client
        .read_resource(ReadResourceRequestParams::new(stale_uri))
        .await
        .expect_err("stale resource generation");
    let ServiceError::McpError(error) = error else {
        panic!("expected MCP error");
    };
    assert_eq!(
        error.data.expect("generation error data"),
        json!({
            "code": "instance_generation_conflict",
            "retryable": false,
            "details": {
                "proxy_endpoint": "127.0.0.1:19010",
                "requested_run_id": RUN_B,
                "current_run_id": RUN_A
            }
        })
    );
    assert!(calls.lock().expect("body calls").is_empty());
    client.cancel().await.expect("close client");
    server.await.expect("server task");
}

#[tokio::test]
async fn resource_read_maps_expected_control_errors_to_stable_json_rpc_error_data() {
    let descriptor = descriptor(19010, RUN_A);
    let probe = Arc::new(BodyProbe {
        descriptor,
        calls: Arc::new(Mutex::new(Vec::new())),
        detail: capture_detail(true),
        body_error: Some(ControlError::new(
            ControlErrorCode::CaptureRevisionConflict,
            "capture revision changed",
            false,
            json!({"capture_id": 0, "expected_revision": 0, "current_revision": 1}),
        )),
    });
    let (client, server) = client_for(body_broker(probe)).await;
    let uri = format!(
        "wirelens://127.0.0.1:19010/runs/{RUN_A}/captures/0/revisions/0/bodies/response/content/decoded?offset=0&length=8192"
    );
    let error = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect_err("typed resource error");
    let ServiceError::McpError(error) = error else {
        panic!("expected MCP error");
    };
    assert_eq!(
        error.data.expect("typed error data"),
        json!({
            "code": "capture_revision_conflict",
            "retryable": false,
            "details": {"capture_id": 0, "expected_revision": 0, "current_revision": 1}
        })
    );
    assert!(!error.message.contains("/tmp"));
    client.cancel().await.expect("close client");
    server.await.expect("server task");
}

#[derive(Clone)]
struct CancellingBodyProbe {
    started: Arc<Notify>,
    cancelled: Arc<Notify>,
    saw_cancel: Arc<AtomicBool>,
}

impl InstanceProbe for CancellingBodyProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move { Ok(describe(descriptor, 1)) })
    }

    fn call<'a>(
        &'a self,
        _descriptor: &'a InstanceDescriptor,
        operation: ControlOperation,
        _client: DeclaredClient,
        _deadline: Instant,
        cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move {
            assert!(matches!(operation, ControlOperation::ReadCaptureBody(_)));
            self.started.notify_one();
            cancelled.cancelled().await;
            self.saw_cancel.store(true, Ordering::Release);
            self.cancelled.notify_one();
            Err(ControlError::cancelled("body resource read cancelled"))
        })
    }
}

#[tokio::test]
async fn broker_disconnect_cancels_an_in_flight_private_body_read() {
    let descriptor = descriptor(19010, RUN_A);
    let probe = Arc::new(CancellingBodyProbe {
        started: Arc::new(Notify::new()),
        cancelled: Arc::new(Notify::new()),
        saw_cancel: Arc::new(AtomicBool::new(false)),
    });
    let broker = Broker::with_dependencies(
        PathBuf::from("/test/.wirelens/run/instances"),
        FakeRegistry::new(vec![descriptor]),
        Arc::clone(&probe),
    );
    let (client, server) = client_for(broker).await;
    let uri = format!(
        "wirelens://127.0.0.1:19010/runs/{RUN_A}/captures/0/revisions/0/bodies/response/content/decoded?offset=0&length=8192"
    );
    let peer = client.peer().clone();
    let read = tokio::spawn(async move {
        peer.read_resource(ReadResourceRequestParams::new(uri))
            .await
    });
    probe.started.notified().await;
    client.cancel().await.expect("disconnect client");
    probe.cancelled.notified().await;
    assert!(probe.saw_cancel.load(Ordering::Acquire));
    read.abort();
    server.await.expect("server task");
}
