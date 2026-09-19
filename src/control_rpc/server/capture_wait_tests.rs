#![cfg(unix)]

use super::{ControlCallContext, ControlRpcHandler, ControlRpcServer};
use crate::{
    control::{CaptureMilestone, WaitForCaptureRequest, capture_query::CaptureQuery},
    control_rpc::{
        framing::ACTIVE_CALL_LIMIT,
        protocol::{
            ControlError, ControlOperation, ControlResult, DeclaredClient, RequestEnvelope,
        },
        test_support::{endpoint, write_payload},
    },
    instance::InstanceIdentity,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{net::UnixStream, sync::Notify};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct BlockingWaitHandler {
    expected: ControlOperation,
    started: Arc<Notify>,
    cancelled: Arc<Notify>,
}

impl ControlRpcHandler for BlockingWaitHandler {
    fn handle(
        &self,
        _context: ControlCallContext,
        operation: ControlOperation,
        cancelled: CancellationToken,
    ) -> impl Future<Output = Result<ControlResult, ControlError>> + Send {
        let expected = self.expected.clone();
        let started = Arc::clone(&self.started);
        let observed = Arc::clone(&self.cancelled);
        async move {
            assert_eq!(operation, expected);
            tokio::spawn(async move {
                cancelled.cancelled().await;
                observed.notify_one();
            });
            started.notify_one();
            std::future::pending::<Result<ControlResult, ControlError>>().await
        }
    }
}

#[tokio::test]
async fn wait_for_capture_socket_disconnect_cancels_promptly_and_releases_private_permit() {
    let identity = InstanceIdentity::new(endpoint()).expect("instance identity");
    let operation = ControlOperation::WaitForCapture(Box::new(WaitForCaptureRequest {
        query: CaptureQuery::default(),
        milestone: CaptureMilestone::ExchangeTerminal,
        timeout_ms: Some(300_000),
    }));
    let started = Arc::new(Notify::new());
    let observed = Arc::new(Notify::new());
    let handler = BlockingWaitHandler {
        expected: operation.clone(),
        started: Arc::clone(&started),
        cancelled: Arc::clone(&observed),
    };
    let server = ControlRpcServer::new(identity.clone(), handler);
    let admission = server.admission.clone();
    let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
    let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

    let request = RequestEnvelope::new(
        "task-9-disconnect".to_owned(),
        identity.run_id().clone(),
        330_000,
        DeclaredClient {
            name: "task-9-private-disconnect".to_owned(),
            version: "1".to_owned(),
        },
        operation,
    )
    .expect("wait request");
    write_payload(
        &mut client_stream,
        &serde_json::to_vec(&request).expect("request JSON"),
    )
    .await;
    started.notified().await;

    let mut remaining = Vec::new();
    for _ in 1..ACTIVE_CALL_LIMIT {
        remaining.push(admission.try_acquire().expect("remaining private permit"));
    }
    assert!(
        admission.try_acquire().is_err(),
        "blocked wait must retain exactly one private call permit"
    );
    drop(remaining);

    drop(client_stream);
    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("server observes disconnect")
        .expect("server task")
        .expect("disconnect is handled");
    tokio::time::timeout(Duration::from_secs(1), observed.notified())
        .await
        .expect("wait handler observes cancellation");

    let restored = (0..ACTIVE_CALL_LIMIT)
        .map(|_| admission.try_acquire().expect("restored private permit"))
        .collect::<Vec<_>>();
    assert_eq!(restored.len(), ACTIVE_CALL_LIMIT);
}
