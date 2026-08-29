use super::{
    ControlCallContext, ControlRpcHandler, ControlRpcServer, validate_peer_identity,
    validate_peer_uid,
};
use crate::{
    control_rpc::{
        framing::{
            CallAdmission, RESPONSE_MAX_BYTES, ResponseSerializationBudget, serialize_json_frame,
        },
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            InstanceScope,
        },
        test_support::{OTHER_RUN_ID, endpoint, read_payload, request_json, write_payload},
    },
    instance::InstanceIdentity,
};
use serde_json::Value;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    net::UnixStream,
    sync::{Notify, mpsc, watch},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum HandlerMode {
    Success,
    ObserveContext(mpsc::Sender<ControlCallContext>),
    WaitForCancellation {
        started: Arc<Notify>,
        observed: Arc<Notify>,
    },
    Hold {
        started: mpsc::Sender<()>,
        release: watch::Receiver<bool>,
    },
    LargeResponse,
}

#[derive(Clone)]
struct TestHandler {
    scope: InstanceScope,
    calls: Arc<AtomicUsize>,
    mode: HandlerMode,
}

impl ControlRpcHandler for TestHandler {
    fn handle(
        &self,
        context: ControlCallContext,
        operation: ControlOperation,
        cancelled: CancellationToken,
    ) -> impl Future<Output = Result<ControlResult, ControlError>> + Send {
        let scope = self.scope.clone();
        let calls = Arc::clone(&self.calls);
        let mode = self.mode.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(operation, ControlOperation::DescribeInstance);
            match mode {
                HandlerMode::Success => {}
                HandlerMode::ObserveContext(sender) => {
                    sender.send(context).await.expect("context receiver");
                }
                HandlerMode::WaitForCancellation { started, observed } => {
                    tokio::spawn(async move {
                        cancelled.cancelled().await;
                        observed.notify_one();
                    });
                    started.notify_one();
                    std::future::pending::<()>().await;
                    unreachable!("pending handler only exits by cancellation");
                }
                HandlerMode::Hold {
                    started,
                    mut release,
                } => {
                    started.send(()).await.expect("started receiver");
                    release
                        .wait_for(|released| *released)
                        .await
                        .expect("release sender");
                }
                HandlerMode::LargeResponse => {
                    return Err(ControlError {
                        code: ControlErrorCode::InstanceUnavailable,
                        message: "large bounded response".to_owned(),
                        retryable: true,
                        details: serde_json::json!({
                            "payload": "x".repeat(6 * 1024 * 1024),
                        }),
                    });
                }
            }
            Ok(ControlResult::DescribeInstance { instance: scope })
        }
    }
}

fn handler(identity: &InstanceIdentity, mode: HandlerMode) -> TestHandler {
    TestHandler {
        scope: InstanceScope {
            proxy_endpoint: identity.proxy_endpoint(),
            run_id: identity.run_id().clone(),
        },
        calls: Arc::new(AtomicUsize::new(0)),
        mode,
    }
}

#[test]
fn peer_identity_requires_the_process_effective_uid() {
    let effective_uid = rustix::process::geteuid().as_raw();

    validate_peer_uid(effective_uid, effective_uid).expect("matching peer UID");
    let error = validate_peer_uid(effective_uid.wrapping_add(1), effective_uid)
        .expect_err("different peer UID");

    assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
}

#[tokio::test]
async fn unix_peer_credentials_report_the_current_process_uid() {
    let (server, _client) = UnixStream::pair().expect("Unix stream pair");

    validate_peer_identity(&server).expect("same-process peer identity");
}

#[tokio::test]
async fn server_dispatches_one_typed_call_then_closes() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let handler = handler(&identity, HandlerMode::Success);
    let server = ControlRpcServer::new(identity.clone(), handler.clone());
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });
    let payload = request_json(identity.run_id().as_str(), 1_000);

    write_payload(&mut client_stream, &payload).await;
    let response: Value =
        serde_json::from_slice(&read_payload(&mut client_stream).await).expect("response JSON");

    assert_eq!(response["protocol_version"], 1);
    assert_eq!(response["request_id"], "request-1");
    assert!(response.get("result").is_some());
    assert!(response.get("error").is_none());
    let mut byte = [0_u8; 1];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), client_stream.read(&mut byte))
            .await
            .expect("server closes promptly")
            .expect("read server EOF"),
        0
    );
    server_task
        .await
        .expect("server task")
        .expect("serve one request");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn server_clamps_deadline_and_preserves_declared_call_context() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let (context_tx, mut context_rx) = mpsc::channel(1);
    let handler = handler(&identity, HandlerMode::ObserveContext(context_tx));
    let server = ControlRpcServer::new(identity.clone(), handler);
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let before_dispatch = Instant::now();
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

    write_payload(
        &mut client_stream,
        &request_json(identity.run_id().as_str(), u64::MAX),
    )
    .await;
    let context = context_rx.recv().await.expect("handler context");

    assert_eq!(context.request_id, "request-1");
    assert_eq!(
        context.declared_client,
        DeclaredClient {
            name: "test-client".to_owned(),
            version: "1.0".to_owned()
        }
    );
    assert!(context.deadline >= before_dispatch + Duration::from_secs(29));
    assert!(context.deadline <= Instant::now() + Duration::from_secs(30));

    let _response = read_payload(&mut client_stream).await;
    server_task
        .await
        .expect("server task")
        .expect("serve one request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_serialization_and_write_remain_inside_the_request_deadline() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let handler = handler(&identity, HandlerMode::LargeResponse);
    let server = ControlRpcServer::new(identity.clone(), handler);
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

    write_payload(
        &mut client_stream,
        &request_json(identity.run_id().as_str(), 250),
    )
    .await;

    let result = tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("the clamped request deadline bounds response completion")
        .expect("server task");
    if let Err(error) = result {
        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_cancels_response_budget_wait_before_a_lease_is_available() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let handler = handler(&identity, HandlerMode::Success);
    let budget = ResponseSerializationBudget::new(RESPONSE_MAX_BYTES);
    let framing_admission = CallAdmission::new(1);
    let held = serialize_json_frame(
        "held",
        RESPONSE_MAX_BYTES,
        budget.clone(),
        framing_admission
            .acquire()
            .await
            .expect("framing call permit"),
    )
    .await
    .expect("held response lease");
    let server =
        ControlRpcServer::new_with_response_budget(identity.clone(), handler.clone(), budget);
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

    write_payload(
        &mut client_stream,
        &request_json(identity.run_id().as_str(), 5_000),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while handler.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("handler returned before budget wait");
    drop(client_stream);

    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("disconnect cancels response budget wait")
        .expect("server task")
        .expect("disconnect handled");
    drop(held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_admits_at_most_thirty_two_calls() {
    const ACTIVE_CALLS: usize = 32;
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let (started_tx, mut started_rx) = mpsc::channel(ACTIVE_CALLS + 1);
    let (release_tx, release_rx) = watch::channel(false);
    let handler = handler(
        &identity,
        HandlerMode::Hold {
            started: started_tx,
            release: release_rx,
        },
    );
    let server = Arc::new(ControlRpcServer::new(identity.clone(), handler.clone()));
    let mut clients = Vec::with_capacity(ACTIVE_CALLS + 1);
    let mut tasks = Vec::with_capacity(ACTIVE_CALLS + 1);

    for _ in 0..=ACTIVE_CALLS {
        let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
        let server = Arc::clone(&server);
        tasks.push(tokio::spawn(async move {
            server.serve_connection(server_stream).await
        }));
        write_payload(
            &mut client_stream,
            &request_json(identity.run_id().as_str(), 30_000),
        )
        .await;
        clients.push(client_stream);
    }

    for _ in 0..ACTIVE_CALLS {
        started_rx.recv().await.expect("admitted handler");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), started_rx.recv())
            .await
            .is_err(),
        "the thirty-third handler must wait for a call permit"
    );

    release_tx.send(true).expect("release handlers");
    tokio::time::timeout(Duration::from_secs(1), started_rx.recv())
        .await
        .expect("thirty-third call is admitted after release")
        .expect("thirty-third handler");
    for client in &mut clients {
        let _response = read_payload(client).await;
    }
    for task in tasks {
        task.await
            .expect("server task")
            .expect("serve admitted request");
    }
    assert_eq!(handler.calls.load(Ordering::SeqCst), ACTIVE_CALLS + 1);
}

#[tokio::test]
async fn stale_run_id_is_rejected_before_handler_dispatch() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let handler = handler(&identity, HandlerMode::Success);
    let server = ControlRpcServer::new(identity.clone(), handler.clone());
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

    write_payload(&mut client_stream, &request_json(OTHER_RUN_ID, 1_000)).await;
    let response: Value =
        serde_json::from_slice(&read_payload(&mut client_stream).await).expect("response JSON");

    assert_eq!(response["error"]["code"], "instance_generation_conflict");
    assert_eq!(response["error"]["retryable"], false);
    assert_eq!(
        response["error"]["details"]["authoritative_identity"]["proxy_endpoint"],
        identity.proxy_endpoint().to_string()
    );
    assert_eq!(
        response["error"]["details"]["authoritative_identity"]["run_id"],
        identity.run_id().as_str()
    );
    server_task
        .await
        .expect("server task")
        .expect("serve rejected request");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn broker_disconnect_cancels_the_dispatched_operation() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let started = Arc::new(Notify::new());
    let observed = Arc::new(Notify::new());
    let handler = handler(
        &identity,
        HandlerMode::WaitForCancellation {
            started: Arc::clone(&started),
            observed: Arc::clone(&observed),
        },
    );
    let server = ControlRpcServer::new(identity.clone(), handler);
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

    write_payload(
        &mut client_stream,
        &request_json(identity.run_id().as_str(), 30_000),
    )
    .await;
    started.notified().await;
    drop(client_stream);

    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("server observes disconnect")
        .expect("server task")
        .expect("disconnect is handled");
    tokio::time::timeout(Duration::from_secs(1), observed.notified())
        .await
        .expect("handler observes cancellation token");
}
