#![cfg(unix)]

use super::{
    BodyCacheKey, BodyJobScheduler, ControlServiceContext, DecodeWorkerCancellation,
    RuntimeBodyServices, RuntimeControlHandler, RuntimeGateway,
    body_test_support::{headers, key, request, scope, status},
};
use crate::{
    capture::{
        BodySide, BodyStreamState, BodyWorkAdmission, CaptureChangeFeed, CaptureChangeKind,
        CaptureSequence, CapturedBodyPreview, CapturedHeaders, DecodeDisplayMode, DecodeKey,
        DecodePolicy, DecodedBytes,
    },
    control::{
        RuntimeReply, RuntimeRequest,
        body::{BodyRepresentation, CaptureBodyMetadataReply, CaptureBodySnapshotReply},
    },
    control_rpc::{
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            InstanceScope,
        },
        server::{ControlCallContext, ControlRpcHandler},
    },
};
use hyper::body::Bytes;
use serde_json::json;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn context(request_id: &str) -> ControlCallContext {
    ControlCallContext {
        request_id: request_id.to_owned(),
        declared_client: DeclaredClient {
            name: "task-10-runtime-test".to_owned(),
            version: "1".to_owned(),
        },
        deadline: std::time::Instant::now() + Duration::from_secs(30),
    }
}

#[test]
fn dropped_decode_future_cancels_its_blocking_worker_unless_completed() {
    let flag = Arc::new(AtomicBool::new(false));
    {
        let _guard = DecodeWorkerCancellation::new(Arc::clone(&flag));
    }
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));

    flag.store(false, std::sync::atomic::Ordering::Release);
    {
        let mut guard = DecodeWorkerCancellation::new(Arc::clone(&flag));
        guard.disarm();
    }
    assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
}

fn metadata_reply(instance: InstanceScope, stream: BodyStreamState) -> CaptureBodyMetadataReply {
    CaptureBodyMetadataReply {
        instance,
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        status: status(stream, 4),
        headers: headers("text/plain"),
        retained_bytes: 4,
    }
}

fn snapshot_reply(instance: InstanceScope, stream: BodyStreamState) -> CaptureBodySnapshotReply {
    CaptureBodySnapshotReply {
        instance,
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        status: status(stream, 4),
        headers: headers("text/plain"),
        retained_bytes: 4,
        preview: CapturedBodyPreview::unbudgeted(Bytes::from_static(b"body")),
    }
}

fn handler(
    feed: CaptureChangeFeed,
    admission: Arc<BodyWorkAdmission>,
) -> (RuntimeControlHandler, super::RuntimeControlReceiver) {
    let (runtime, receiver) = RuntimeGateway::channel(16);
    (
        RuntimeControlHandler::new(ControlServiceContext {
            runtime,
            capture_changes: feed,
            body_work: admission,
            audit: crate::control::audit::InstanceAudit::default(),
            config_source: None,
        }),
        receiver,
    )
}

fn spawn_read(
    handler: RuntimeControlHandler,
    representation: BodyRepresentation,
) -> tokio::task::JoinHandle<Result<ControlResult, ControlError>> {
    tokio::spawn(async move {
        handler
            .handle(
                context("task-10-read"),
                ControlOperation::ReadCaptureBody(Box::new(request(representation, 0, 8_192))),
                CancellationToken::new(),
            )
            .await
    })
}

#[tokio::test]
async fn change_published_during_decode_cannot_resurrect_a_stale_cache_entry() {
    let feed = CaptureChangeFeed::new();
    let admission = Arc::new(BodyWorkAdmission::new());
    let (handler, mut receiver) = handler(feed.clone(), admission);
    let read = spawn_read(handler.clone(), BodyRepresentation::Decoded);
    let instance = scope(19010);

    for _ in 0..2 {
        let command = receiver.recv().await.expect("metadata request");
        command
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), BodyStreamState::Complete),
            ))))
            .expect("metadata reply");
    }
    let command = receiver.recv().await.expect("snapshot request");
    feed.publish(CaptureSequence::new(7), 4, CaptureChangeKind::RecordUpdated);
    command
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            snapshot_reply(instance, BodyStreamState::Complete),
        ))))
        .expect("snapshot reply");

    read.await
        .expect("read join")
        .expect("decoded body still serves its validated snapshot");
    assert_eq!(handler.body_jobs.cache.lock().test_snapshot().entries, 0);
}

#[tokio::test]
async fn stale_revision_and_missing_capture_are_returned_by_runtime_authority() {
    for (error_code, details) in [
        (
            ControlErrorCode::CaptureRevisionConflict,
            json!({"capture_id": 7, "expected_revision": 3, "current_revision": 4}),
        ),
        (ControlErrorCode::CaptureNotFound, json!({"capture_id": 7})),
    ] {
        let admission = Arc::new(BodyWorkAdmission::new());
        let (handler, mut receiver) = handler(CaptureChangeFeed::new(), admission);
        let read = spawn_read(handler, BodyRepresentation::Decoded);
        let command = receiver.recv().await.expect("metadata request");
        assert_eq!(
            command.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id: CaptureSequence::new(7),
                side: BodySide::Response,
            }
        );
        command
            .reply
            .send(Err(ControlError::new(
                error_code,
                "runtime body authority rejected read",
                false,
                details.clone(),
            )))
            .expect("metadata error reply");
        let error = read.await.expect("read join").expect_err("body read error");
        assert_eq!(error.code, error_code);
        assert_eq!(error.details, details);
    }
}

#[tokio::test]
async fn retention_loss_after_queue_wait_never_serves_or_caches_the_old_snapshot() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = admission
        .try_admit_mcp(1)
        .expect("first queued lease")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first active lease");
    let second = admission
        .try_admit_mcp(1)
        .expect("second queued lease")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second active lease");
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), Arc::clone(&admission));
    let read = spawn_read(handler, BodyRepresentation::Decoded);
    let metadata = receiver.recv().await.expect("metadata request");
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
            metadata_reply(scope(19010), BodyStreamState::Complete),
        ))))
        .expect("metadata reply");

    admission.test_wait_for_queued(1).await;
    assert_eq!(admission.test_snapshot().active, 2);
    assert_eq!(admission.test_snapshot().queued, 1);
    drop(first);

    let metadata = receiver.recv().await.expect("post-active revalidation");
    assert!(matches!(
        metadata.request,
        RuntimeRequest::GetCaptureBodyMetadata {
            capture_id,
            side: BodySide::Response
        } if capture_id == CaptureSequence::new(7)
    ));
    metadata
        .reply
        .send(Err(ControlError::new(
            ControlErrorCode::CaptureNotFound,
            "capture was evicted while body work was queued",
            false,
            json!({"capture_id": 7}),
        )))
        .expect("retention loss reply");
    let error = read.await.expect("read join").expect_err("retention loss");
    assert_eq!(error.code, ControlErrorCode::CaptureNotFound);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert_eq!(admission.test_snapshot().queued_bytes, 0);
    drop(second);
    assert_eq!(admission.test_snapshot().active, 0);
}

async fn answer_one_decoded_read(
    receiver: &mut super::RuntimeControlReceiver,
    instance: &InstanceScope,
    stream: BodyStreamState,
) {
    for phase in ["pre-admission", "post-active"] {
        let metadata = receiver.recv().await.expect("metadata request");
        assert_eq!(
            metadata.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id: CaptureSequence::new(7),
                side: BodySide::Response,
            }
        );
        metadata
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), stream),
            ))))
            .unwrap_or_else(|_| panic!("{phase} metadata reply"));
    }
    let snapshot = receiver.recv().await.expect("snapshot request");
    assert_eq!(
        snapshot.request,
        RuntimeRequest::GetCaptureBodySnapshot {
            capture_id: CaptureSequence::new(7),
            expected_revision: 3,
            side: BodySide::Response,
        }
    );
    snapshot
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            snapshot_reply(instance.clone(), stream),
        ))))
        .expect("snapshot reply");
}

#[tokio::test]
async fn live_decoded_reads_never_use_the_terminal_cache() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), admission);
    let instance = scope(19010);
    for _ in 0..2 {
        let read = spawn_read(handler.clone(), BodyRepresentation::Decoded);
        answer_one_decoded_read(&mut receiver, &instance, BodyStreamState::Streaming).await;
        let result = read.await.expect("read join").expect("live decoded read");
        let ControlResult::ReadCaptureBody { page, .. } = result else {
            panic!("expected body result");
        };
        assert_eq!(page.content.as_ref(), b"body");
    }
}

#[tokio::test]
async fn terminal_decoded_read_hits_cache_but_revalidates_metadata_before_serving() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), admission);
    let instance = scope(19010);
    let first = spawn_read(handler.clone(), BodyRepresentation::Decoded);
    answer_one_decoded_read(&mut receiver, &instance, BodyStreamState::Complete).await;
    first
        .await
        .expect("first join")
        .expect("first terminal read");

    let second = spawn_read(handler, BodyRepresentation::Decoded);
    for phase in ["pre-admission", "post-active"] {
        let metadata = receiver.recv().await.expect("cached metadata revalidation");
        assert!(matches!(
            metadata.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id,
                side: BodySide::Response
            } if capture_id == CaptureSequence::new(7)
        ));
        metadata
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), BodyStreamState::Complete),
            ))))
            .unwrap_or_else(|_| panic!("{phase} cached metadata reply"));
    }
    let result = second.await.expect("second join").expect("cached read");
    let ControlResult::ReadCaptureBody { page, .. } = result else {
        panic!("expected body result");
    };
    assert_eq!(page.content.as_ref(), b"body");
    assert!(
        receiver.try_recv().is_err(),
        "cache hit must not clone a body snapshot"
    );
}

#[tokio::test]
async fn terminal_cache_hits_charge_the_decoded_representation_while_queued() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = admission
        .try_admit_mcp(0)
        .expect("first active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first active");
    let second = admission
        .try_admit_mcp(0)
        .expect("second active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second active");
    let instance = scope(19010);
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), Arc::clone(&admission));
    handler.body_jobs.cache.lock().insert_decoded(
        BodyCacheKey {
            instance: instance.clone(),
            capture_id: CaptureSequence::new(7),
            capture_revision: 3,
            side: BodySide::Response,
            representation: BodyRepresentation::Decoded,
        },
        DecodedBytes::new(Bytes::from_static(b"decoded"), Vec::new(), false),
    );

    let read = spawn_read(handler, BodyRepresentation::Decoded);
    let metadata = receiver.recv().await.expect("metadata request");
    let mut reply = metadata_reply(instance.clone(), BodyStreamState::Complete);
    reply.retained_bytes = 1;
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(reply))))
        .expect("metadata reply");
    admission.test_wait_for_queued(1).await;
    assert_eq!(admission.test_snapshot().queued_bytes, b"decoded".len());

    drop(first);
    let metadata = receiver.recv().await.expect("post-active metadata request");
    let mut reply = metadata_reply(instance, BodyStreamState::Complete);
    reply.retained_bytes = 1;
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(reply))))
        .expect("post-active metadata reply");
    read.await
        .expect("cached read join")
        .expect("cached decoded read");
    drop(second);
}

#[tokio::test]
async fn cache_availability_changes_trigger_re_admission_before_snapshot_work() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = admission
        .try_admit_mcp(0)
        .expect("first active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first active");
    let second = admission
        .try_admit_mcp(0)
        .expect("second active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second active");
    let instance = scope(19010);
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), Arc::clone(&admission));
    let cache_key = key(instance.clone(), 3, BodySide::Response);
    handler.body_jobs.cache.lock().insert_decoded(
        cache_key.clone(),
        DecodedBytes::new(Bytes::from_static(b"decoded"), Vec::new(), false),
    );

    let read = spawn_read(handler.clone(), BodyRepresentation::Decoded);
    let metadata = receiver.recv().await.expect("initial metadata");
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
            metadata_reply(instance.clone(), BodyStreamState::Complete),
        ))))
        .expect("initial metadata reply");
    admission.test_wait_for_queued(1).await;
    handler.body_jobs.cache.lock().remove(&cache_key);

    drop(first);
    for phase in ["post-cache-loss", "post-readmission"] {
        let metadata = receiver.recv().await.expect(phase);
        metadata
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), BodyStreamState::Complete),
            ))))
            .unwrap_or_else(|_| panic!("{phase} metadata reply"));
    }
    let snapshot = receiver.recv().await.expect("snapshot after readmission");
    snapshot
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            snapshot_reply(instance, BodyStreamState::Complete),
        ))))
        .expect("snapshot reply");

    read.await
        .expect("read join")
        .expect("decoded miss after cache loss");
    drop(second);
}

#[test]
fn scheduler_owns_the_shared_admission_and_a_thirty_two_mib_cache() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let (runtime, _receiver) = RuntimeGateway::channel(1);
    let scheduler = BodyJobScheduler::new(
        scope(19010),
        runtime,
        CaptureChangeFeed::new(),
        Arc::clone(&admission),
    );
    assert!(Arc::ptr_eq(&admission, scheduler.test_admission()));
    assert_eq!(scheduler.test_cache_capacity_bytes(), 32 * 1_024 * 1_024);
}

#[tokio::test]
async fn runtime_body_services_share_one_exact_admission_between_tui_and_mcp() {
    let shutdown = CancellationToken::new();
    let services = RuntimeBodyServices::start(DecodePolicy::default(), shutdown.clone());
    let (runtime, _receiver) = RuntimeGateway::channel(1);
    let context = services.control_context(runtime, CaptureChangeFeed::new());
    assert!(Arc::ptr_eq(
        services.decode.client.test_body_work_admission(),
        &context.body_work,
    ));
    assert!(Arc::ptr_eq(&services.admission, &context.body_work));

    let first = context
        .body_work
        .try_admit_mcp(1)
        .expect("first shared queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first shared active");
    let second = context
        .body_work
        .try_admit_mcp(1)
        .expect("second shared queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second shared active");
    let queued = (0..8)
        .map(|_| {
            context
                .body_work
                .try_admit_mcp(1)
                .expect("shared queue slot")
        })
        .collect::<Vec<_>>();
    let mcp_error = context
        .body_work
        .try_admit_mcp(1)
        .expect_err("MCP sees shared saturation");
    assert_eq!(mcp_error.code, ControlErrorCode::ResourceLimit);
    assert!(!services.decode.client.request(
        DecodeKey {
            sequence: CaptureSequence::new(99),
            side: BodySide::Response,
            revision: 1,
            mode: DecodeDisplayMode::Response,
        },
        CapturedBodyPreview::unbudgeted(Bytes::from_static(b"body")),
        CapturedHeaders::unbudgeted(Arc::from([])),
    ));
    drop((queued, first, second));

    shutdown.cancel();
    services
        .decode
        .task
        .await
        .expect("decode service join")
        .expect("decode service shutdown");
}
