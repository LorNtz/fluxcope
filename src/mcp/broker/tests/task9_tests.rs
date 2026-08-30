use super::*;
use crate::{
    control::{
        CaptureMilestone, WaitForCaptureRequest, WaitForCaptureResult, capture_query::CaptureQuery,
    },
    control_rpc::protocol::ControlOperation,
    instance_registry::RegistryScan,
};
use rmcp::{ServiceError, model::CallToolRequestParams};
use serde_json::{Map, Value, json};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().expect("tool argument object").clone()
}

fn tool_json(result: &rmcp::model::CallToolResult) -> Value {
    result
        .structured_content
        .clone()
        .expect("structured tool result")
}

fn wait_scope(descriptor: &InstanceDescriptor) -> InstanceScope {
    InstanceScope {
        proxy_endpoint: descriptor.proxy_endpoint(),
        run_id: descriptor.run_id().clone(),
    }
}

#[derive(Clone)]
struct ReturningWaitProbe {
    calls: Arc<Mutex<Vec<WaitForCaptureRequest>>>,
}

impl InstanceProbe for ReturningWaitProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move { Ok(describe(descriptor, 0)) })
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
            let ControlOperation::WaitForCapture(request) = operation else {
                return Err(ControlError::invalid_argument("expected wait operation"));
            };
            self.calls.lock().expect("wait calls").push(*request);
            Ok(ControlResult::WaitForCapture {
                instance: wait_scope(descriptor),
                result: WaitForCaptureResult {
                    matched: false,
                    capture: None,
                },
            })
        })
    }
}

#[tokio::test]
async fn wait_for_capture_child_transport_normalizes_timeout_and_returns_unmatched_success() {
    let selected = descriptor(19809, RUN_A);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let broker = Broker::with_dependencies(
        PathBuf::from("/test/.wirelens/run/instances"),
        FakeRegistry::new(vec![selected.clone()]),
        Arc::new(ReturningWaitProbe {
            calls: Arc::clone(&calls),
        }),
    );
    let (client_transport, server_transport) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let service = broker.serve(server_transport).await.expect("serve broker");
        service.waiting().await.expect("broker shutdown");
    });
    let client = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("wait-routing-test", "1.0"),
    )
    .with_protocol_version(ProtocolVersion::LATEST)
    .serve(client_transport)
    .await
    .expect("initialize client");

    for (timeout, expected_timeout) in [(None, 30_000), (Some(u64::MAX), 300_000)] {
        let mut input = json!({
            "instance": {
                "proxy_endpoint": selected.proxy_endpoint(),
                "run_id": selected.run_id()
            },
            "query": {},
            "milestone": "exchange_terminal"
        });
        if let Some(timeout) = timeout {
            input["timeout_ms"] = json!(timeout);
        }
        let result = client
            .call_tool(
                CallToolRequestParams::new("wait_for_capture").with_arguments(arguments(input)),
            )
            .await
            .expect("normal wait timeout is a successful tool result");
        assert_eq!(
            tool_json(&result),
            json!({
                "instance": {
                    "proxy_endpoint": selected.proxy_endpoint(),
                    "run_id": selected.run_id()
                },
                "matched": false,
                "capture": null
            })
        );
        let routed = calls.lock().expect("wait calls");
        let routed = routed.last().expect("routed wait");
        assert_eq!(routed.query, CaptureQuery::default());
        assert_eq!(routed.milestone, CaptureMilestone::ExchangeTerminal);
        assert_eq!(routed.timeout_ms, Some(expected_timeout));
    }

    let calls_before_zero = calls.lock().expect("wait calls").len();
    let error = client
        .call_tool(
            CallToolRequestParams::new("wait_for_capture").with_arguments(arguments(json!({
                "instance": {
                    "proxy_endpoint": selected.proxy_endpoint(),
                    "run_id": selected.run_id()
                },
                "milestone": "request_seen",
                "timeout_ms": 0
            }))),
        )
        .await
        .expect_err("zero timeout is invalid MCP input");
    let ServiceError::McpError(error) = error else {
        panic!("expected typed MCP error");
    };
    assert_eq!(
        error.data.expect("typed MCP error data")["code"],
        json!("invalid_argument")
    );
    assert_eq!(calls.lock().expect("wait calls").len(), calls_before_zero);

    client.cancel().await.expect("close client");
    server.await.expect("server task");
}

#[derive(Clone)]
struct CancellingWaitProbe {
    started: Arc<Notify>,
    observed: Arc<Notify>,
}

impl InstanceProbe for CancellingWaitProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move { Ok(describe(descriptor, 0)) })
    }

    fn call<'a>(
        &'a self,
        _descriptor: &'a InstanceDescriptor,
        operation: ControlOperation,
        _client: DeclaredClient,
        _deadline: Instant,
        cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        let started = Arc::clone(&self.started);
        let observed = Arc::clone(&self.observed);
        Box::pin(async move {
            assert!(matches!(operation, ControlOperation::WaitForCapture(_)));
            tokio::spawn(async move {
                cancelled.cancelled().await;
                observed.notify_one();
            });
            started.notify_one();
            std::future::pending::<Result<ControlResult, ControlError>>().await
        })
    }
}

#[tokio::test]
async fn wait_for_capture_child_disconnect_cancels_and_restores_public_call_permit() {
    let selected = descriptor(19810, RUN_A);
    let started = Arc::new(Notify::new());
    let observed = Arc::new(Notify::new());
    let broker = Broker::with_dependencies(
        PathBuf::from("/test/.wirelens/run/instances"),
        FakeRegistry::new(vec![selected.clone()]),
        Arc::new(CancellingWaitProbe {
            started: Arc::clone(&started),
            observed: Arc::clone(&observed),
        }),
    );
    let admission = Arc::clone(&broker.call_admission);
    let (client_transport, server_transport) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let service = broker.serve(server_transport).await.expect("serve broker");
        service.waiting().await.expect("broker shutdown");
    });
    let client = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("wait-cancellation-test", "1.0"),
    )
    .serve(client_transport)
    .await
    .expect("initialize client");
    let caller = client.clone();
    let call = tokio::spawn(async move {
        caller
            .call_tool(
                CallToolRequestParams::new("wait_for_capture").with_arguments(arguments(json!({
                    "instance": {
                        "proxy_endpoint": selected.proxy_endpoint(),
                        "run_id": selected.run_id()
                    },
                    "milestone": "request_seen",
                    "timeout_ms": 300_000
                }))),
            )
            .await
    });
    started.notified().await;
    assert_eq!(admission.available_permits(), 31);

    client.cancel().await.expect("disconnect client");
    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("broker observes child disconnect")
        .expect("server task");
    tokio::time::timeout(Duration::from_secs(1), observed.notified())
        .await
        .expect("private wait observes cancellation");
    let _ = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .expect("public wait call terminates")
        .expect("call task");
    assert_eq!(admission.available_permits(), 32);
}

struct ReplacingRegistry {
    old: InstanceDescriptor,
    replacement: InstanceDescriptor,
    replaced: Arc<AtomicBool>,
}

impl RegistryAccess for ReplacingRegistry {
    fn scan_all(&self) -> io::Result<RegistryScan> {
        Ok(RegistryScan {
            candidates: vec![if self.replaced.load(Ordering::SeqCst) {
                self.replacement.clone()
            } else {
                self.old.clone()
            }],
            rejected: Vec::new(),
            omitted: 0,
        })
    }

    fn read_endpoint(&self, endpoint: SocketAddr) -> io::Result<RegistryScan> {
        let current = if self.replaced.load(Ordering::SeqCst) {
            &self.replacement
        } else {
            &self.old
        };
        Ok(RegistryScan {
            candidates: (current.proxy_endpoint() == endpoint)
                .then(|| current.clone())
                .into_iter()
                .collect(),
            rejected: Vec::new(),
            omitted: 0,
        })
    }

    fn prune_batch_if_current(
        &self,
        _descriptors: &[InstanceDescriptor],
        _deadline: Instant,
        _cancelled: &CancellationToken,
    ) -> io::Result<usize> {
        Ok(0)
    }
}

struct ReplacingWaitProbe {
    replaced: Arc<AtomicBool>,
    wait_targets: Mutex<Vec<RunId>>,
}

impl InstanceProbe for ReplacingWaitProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move { Ok(describe(descriptor, 0)) })
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
            assert!(matches!(operation, ControlOperation::WaitForCapture(_)));
            self.wait_targets
                .lock()
                .expect("wait targets")
                .push(descriptor.run_id().clone());
            self.replaced.store(true, Ordering::SeqCst);
            Err(ControlError::instance_unavailable(
                "old instance exited during wait",
            ))
        })
    }
}

#[tokio::test]
async fn wait_for_capture_endpoint_replacement_returns_generation_conflict_without_retargeting() {
    let old = descriptor(19811, RUN_A);
    let replacement = descriptor(19811, RUN_B);
    let replaced = Arc::new(AtomicBool::new(false));
    let registry = Arc::new(ReplacingRegistry {
        old: old.clone(),
        replacement: replacement.clone(),
        replaced: Arc::clone(&replaced),
    });
    let probe = Arc::new(ReplacingWaitProbe {
        replaced,
        wait_targets: Mutex::new(Vec::new()),
    });
    let broker = Broker::with_dependencies(
        PathBuf::from("/test/.wirelens/run/instances"),
        registry,
        Arc::clone(&probe),
    );
    let (client_transport, server_transport) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let service = broker.serve(server_transport).await.expect("serve broker");
        service.waiting().await.expect("broker shutdown");
    });
    let client = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("wait-generation-test", "1.0"),
    )
    .serve(client_transport)
    .await
    .expect("initialize client");

    let error = client
        .call_tool(
            CallToolRequestParams::new("wait_for_capture").with_arguments(arguments(json!({
                "instance": {
                    "proxy_endpoint": old.proxy_endpoint(),
                    "run_id": old.run_id()
                },
                "milestone": "request_seen",
                "timeout_ms": 30_000
            }))),
        )
        .await
        .expect_err("old generation wait must not be retargeted");
    let ServiceError::McpError(error) = error else {
        panic!("expected typed MCP error");
    };
    let data = error.data.expect("typed generation conflict data");
    assert_eq!(data["code"], json!("instance_generation_conflict"));
    assert_eq!(data["retryable"], json!(false));
    assert_eq!(data["details"]["requested_run_id"], json!(RUN_A));
    assert_eq!(data["details"]["current_run_id"], json!(RUN_B));
    assert_eq!(
        probe.wait_targets.lock().expect("wait targets").as_slice(),
        &[old.run_id().clone()],
        "the replacement generation must never receive the old wait"
    );

    client.cancel().await.expect("close client");
    server.await.expect("server task");
}
