#![cfg(unix)]

use super::{CaptureSearchAdmission, ControlServiceContext, RuntimeControlHandler, RuntimeGateway};
use crate::{
    capture::{
        BodyStreamState, BodyWorkAdmission, CaptureChangeFeed, CaptureChangeKind, CapturePolicy,
        CapturePublisher, CaptureRecord, CaptureSequence, CaptureSnapshot, CaptureSnapshotMode,
        CapturedExchange, RequestCaptureInput,
    },
    control::{
        CaptureMilestone, InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest,
        WaitForCaptureRequest, WaitForCaptureResult,
        capture_query::{
            CaptureHeaderFilter, CaptureQuery, CaptureSearchBatch, CaptureStatusFilter,
        },
    },
    control_rpc::{
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            InstanceScope,
        },
        server::{ControlCallContext, ControlRpcHandler},
    },
    instance::InstanceIdentity,
    settings::{ConfigMode, PersistenceMode},
};
use http::HeaderMap;
use hyper::Method;
use serde_json::json;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct BlockingGate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl BlockingGate {
    fn wait(&self) {
        let mut open = self.open.lock().expect("gate");
        while !*open {
            open = self.changed.wait(open).expect("gate");
        }
    }

    fn release(&self) {
        *self.open.lock().expect("gate") = true;
        self.changed.notify_all();
    }
}

fn scope() -> InstanceScope {
    let identity = InstanceIdentity::new("127.0.0.1:19029".parse().expect("endpoint"))
        .expect("instance identity");
    InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    }
}

fn context(request_id: &str) -> ControlCallContext {
    ControlCallContext {
        request_id: request_id.to_owned(),
        declared_client: DeclaredClient {
            name: "task-9-runtime-test".to_owned(),
            version: "1".to_owned(),
        },
        deadline: std::time::Instant::now() + Duration::from_secs(330),
    }
}

fn completed_snapshot(sequence: u64) -> CaptureSnapshot {
    CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: Method::GET,
        uri: format!("https://example.test/{sequence}"),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![],
        res_headers: vec![("X-Ready".to_owned(), "yes".to_owned())],
        req_body: None,
        res_body: None,
    })
    .snapshot(CaptureSnapshotMode::MetadataOnly)
}

fn pending_snapshot(sequence: u64) -> CaptureSnapshot {
    let mut snapshot = completed_snapshot(sequence);
    snapshot.response = None;
    snapshot.request_body.status.stream = BodyStreamState::Streaming;
    snapshot.response_body.status.stream = BodyStreamState::Pending;
    snapshot.timing.time_to_response = None;
    snapshot.timing.total_duration = None;
    snapshot
}

fn wait_request(
    query: CaptureQuery,
    milestone: CaptureMilestone,
    timeout_ms: Option<u64>,
) -> ControlOperation {
    ControlOperation::WaitForCapture(Box::new(WaitForCaptureRequest {
        query,
        milestone,
        timeout_ms,
    }))
}

fn handler(
    feed: CaptureChangeFeed,
    capacity: usize,
) -> (
    RuntimeControlHandler,
    super::RuntimeControlReceiver,
    Arc<CaptureSearchAdmission>,
) {
    let (runtime, receiver) = RuntimeGateway::channel(capacity);
    let handler = RuntimeControlHandler::new(ControlServiceContext {
        runtime,
        capture_changes: feed,
        body_work: Arc::new(BodyWorkAdmission::default()),
        audit: crate::control::audit::InstanceAudit::default(),
        config_source: None,
    });
    let admission = Arc::clone(&handler.capture_searches);
    (handler, receiver, admission)
}

fn spawn_wait(
    handler: RuntimeControlHandler,
    request_id: &'static str,
    operation: ControlOperation,
    cancelled: CancellationToken,
) -> tokio::task::JoinHandle<Result<ControlResult, ControlError>> {
    tokio::spawn(async move {
        handler
            .handle(context(request_id), operation, cancelled)
            .await
    })
}

fn status_reply(instance: &InstanceScope, retained_capture_count: usize) -> RuntimeReply {
    RuntimeReply::Instance(Box::new(InstanceRuntimeSnapshot {
        instance: instance.clone(),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        recording_enabled: true,
        retained_capture_count,
        settings_revision: 7,
        mapping: Default::default(),
        capture_store: Default::default(),
        metrics: Default::default(),
    }))
}

async fn reply_initial_search(
    receiver: &mut super::RuntimeControlReceiver,
    instance: &InstanceScope,
    snapshots: Vec<CaptureSnapshot>,
) {
    loop {
        let command = receiver.recv().await.expect("initial wait command");
        match command.request {
            RuntimeRequest::GetStatus => command
                .reply
                .send(Ok(status_reply(instance, snapshots.len())))
                .expect("status reply receiver"),
            RuntimeRequest::GetCaptureSearchBatch { cursor, max_rows } => {
                assert_eq!(cursor, None);
                assert_eq!(max_rows, 32);
                command
                    .reply
                    .send(Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
                        snapshots,
                        next_cursor: None,
                    })))
                    .expect("search reply receiver");
                return;
            }
            other => panic!("unexpected initial wait command: {other:?}"),
        }
    }
}

fn assert_matched(
    result: Result<ControlResult, ControlError>,
    expected_scope: &InstanceScope,
    expected_sequence: u64,
) {
    let ControlResult::WaitForCapture { instance, result } = result.expect("wait result") else {
        panic!("expected wait result");
    };
    assert_eq!(&instance, expected_scope);
    assert!(result.matched);
    assert_eq!(
        result.capture.expect("matching capture").capture_sequence,
        CaptureSequence::new(expected_sequence)
    );
}

#[test]
fn wait_for_capture_milestones_derive_truth_from_one_snapshot() {
    let request = pending_snapshot(1);
    assert!(CaptureMilestone::RequestSeen.is_satisfied_by(&request));
    assert!(!CaptureMilestone::ResponseStarted.is_satisfied_by(&request));
    assert!(!CaptureMilestone::ExchangeTerminal.is_satisfied_by(&request));

    let mut response_without_metadata = request.clone();
    response_without_metadata.timing.time_to_response = Some(Duration::from_millis(3));
    response_without_metadata
        .metadata_truncation
        .response_headers = true;
    assert!(response_without_metadata.response.is_none());
    assert!(CaptureMilestone::ResponseStarted.is_satisfied_by(&response_without_metadata));

    for terminal in [
        BodyStreamState::Complete,
        BodyStreamState::Failed,
        BodyStreamState::Cancelled,
    ] {
        let mut snapshot = completed_snapshot(2);
        snapshot.request_body.status.stream = terminal;
        snapshot.response_body.status.stream = terminal;
        assert!(
            CaptureMilestone::ExchangeTerminal.is_satisfied_by(&snapshot),
            "{terminal:?} is terminal"
        );
    }

    let mut half_terminal = completed_snapshot(3);
    half_terminal.request_body.status.stream = BodyStreamState::Complete;
    half_terminal.response_body.status.stream = BodyStreamState::Streaming;
    assert!(!CaptureMilestone::ExchangeTerminal.is_satisfied_by(&half_terminal));
}

#[tokio::test]
async fn wait_for_capture_subscribes_before_initial_query_without_missing_the_race() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed.clone(), 4);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "subscribe-before-query",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(30_000),
        ),
        CancellationToken::new(),
    );

    loop {
        let command = receiver.recv().await.expect("initial command");
        match command.request {
            RuntimeRequest::GetStatus => command
                .reply
                .send(Ok(status_reply(&instance, 0)))
                .expect("status receiver"),
            RuntimeRequest::GetCaptureSearchBatch { .. } => {
                feed.publish(CaptureSequence::new(7), 0, CaptureChangeKind::Admitted);
                command
                    .reply
                    .send(Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
                        snapshots: Vec::new(),
                        next_cursor: None,
                    })))
                    .expect("batch receiver");
                break;
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    let changed = receiver.recv().await.expect("changed capture recheck");
    assert_eq!(
        changed.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(7),
            expected_revision: None,
        }
    );
    changed
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: completed_snapshot(7),
            },
        ))))
        .expect("capture receiver");

    assert_matched(task.await.expect("wait task"), &instance, 7);
}

#[tokio::test]
async fn wait_for_capture_returns_newest_terminal_match_not_newer_pending_match() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed, 4);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "newest-satisfied",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::ExchangeTerminal,
            Some(30_000),
        ),
        CancellationToken::new(),
    );

    reply_initial_search(
        &mut receiver,
        &instance,
        vec![pending_snapshot(9), completed_snapshot(8)],
    )
    .await;

    assert_matched(task.await.expect("wait task"), &instance, 8);
}

#[tokio::test(start_paused = true)]
async fn wait_for_capture_response_filters_do_not_match_before_response_metadata_exists() {
    let filters = [
        CaptureQuery {
            status: Some(CaptureStatusFilter {
                exact: Some(200),
                minimum: None,
                maximum: None,
            }),
            ..CaptureQuery::default()
        },
        CaptureQuery {
            header: Some(CaptureHeaderFilter {
                name: "x-ready".to_owned(),
                value: Some("yes".to_owned()),
            }),
            ..CaptureQuery::default()
        },
    ];

    for (index, query) in filters.into_iter().enumerate() {
        let feed = CaptureChangeFeed::new();
        let (handler, mut receiver, _) = handler(feed, 4);
        let instance = scope();
        let task = spawn_wait(
            handler,
            "response-filter-before-response",
            wait_request(query, CaptureMilestone::RequestSeen, Some(1_000)),
            CancellationToken::new(),
        );

        reply_initial_search(
            &mut receiver,
            &instance,
            vec![pending_snapshot(4 + index as u64)],
        )
        .await;
        tokio::time::advance(Duration::from_secs(1)).await;

        let ControlResult::WaitForCapture {
            instance: actual,
            result,
        } = task.await.expect("wait task").expect("timeout success")
        else {
            panic!("expected wait result");
        };
        assert_eq!(actual, instance);
        assert_eq!(
            result,
            WaitForCaptureResult {
                matched: false,
                capture: None,
            }
        );
    }
}

#[tokio::test]
async fn wait_for_capture_rechecks_only_changed_sequence_and_releases_capacity_while_asleep() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, admission) = handler(feed.clone(), 1);
    let runtime = handler.runtime.clone();
    let instance = scope();
    let task = spawn_wait(
        handler,
        "changed-only",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(30_000),
        ),
        CancellationToken::new(),
    );

    reply_initial_search(&mut receiver, &instance, Vec::new()).await;
    tokio::task::yield_now().await;
    assert_eq!(admission.available_permits_for_test(), 4);

    let status = tokio::spawn({
        let instance = instance.clone();
        async move {
            let reply = runtime
                .request(RuntimeRequest::GetStatus, CancellationToken::new())
                .await
                .expect("unrelated status reply");
            assert_eq!(reply.instance().instance, instance);
        }
    });
    let command = receiver
        .recv()
        .await
        .expect("status command while wait sleeps");
    assert_eq!(command.request, RuntimeRequest::GetStatus);
    command
        .reply
        .send(Ok(status_reply(&instance, 0)))
        .expect("status receiver");
    status.await.expect("status task");

    feed.publish(
        CaptureSequence::new(12),
        3,
        CaptureChangeKind::RecordUpdated,
    );
    let command = receiver.recv().await.expect("changed capture command");
    assert_eq!(
        command.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(12),
            expected_revision: None,
        },
        "a feed update must not trigger a second full-store scan"
    );
    command
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: completed_snapshot(12),
            },
        ))))
        .expect("capture receiver");

    assert_matched(task.await.expect("wait task"), &instance, 12);
    assert_eq!(admission.available_permits_for_test(), 4);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn wait_for_capture_acquires_search_admission_before_requesting_each_initial_batch() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, admission) = handler(feed, 2);
    let instance = scope();
    let cancelled = CancellationToken::new();
    let task = spawn_wait(
        handler,
        "initial-batch-admission",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(30_000),
        ),
        cancelled.clone(),
    );

    let status = receiver.recv().await.expect("initial status command");
    assert_eq!(status.request, RuntimeRequest::GetStatus);
    let blocker_cancelled = CancellationToken::new();
    let mut blockers = Vec::with_capacity(4);
    for _ in 0..4 {
        blockers.push(
            admission
                .acquire(&blocker_cancelled)
                .await
                .expect("consume search permit"),
        );
    }
    status
        .reply
        .send(Ok(status_reply(&instance, 0)))
        .expect("status reply receiver");
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    assert!(
        receiver.try_recv().is_err(),
        "runtime batches must wait for search admission"
    );

    drop(blockers);
    let batch = receiver.recv().await.expect("admitted initial batch");
    assert!(matches!(
        batch.request,
        RuntimeRequest::GetCaptureSearchBatch { .. }
    ));
    batch
        .reply
        .send(Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
            snapshots: Vec::new(),
            next_cursor: None,
        })))
        .expect("batch reply receiver");
    cancelled.cancel();
    let error = task.await.expect("wait task").expect_err("cancelled wait");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
}

#[tokio::test]
async fn wait_for_capture_initial_snapshot_watermark_skips_queued_older_revisions() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed.clone(), 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "initial-revision-watermark",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::ExchangeTerminal,
            Some(30_000),
        ),
        CancellationToken::new(),
    );

    let mut initial = pending_snapshot(52);
    initial.revision = 5;
    loop {
        let command = receiver.recv().await.expect("initial wait command");
        match command.request {
            RuntimeRequest::GetStatus => command
                .reply
                .send(Ok(status_reply(&instance, 1)))
                .expect("status reply receiver"),
            RuntimeRequest::GetCaptureSearchBatch { .. } => {
                feed.publish(
                    CaptureSequence::new(52),
                    1,
                    CaptureChangeKind::RecordUpdated,
                );
                feed.publish(
                    CaptureSequence::new(52),
                    2,
                    CaptureChangeKind::RecordUpdated,
                );
                command
                    .reply
                    .send(Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
                        snapshots: vec![initial],
                        next_cursor: None,
                    })))
                    .expect("batch reply receiver");
                break;
            }
            other => panic!("unexpected initial command: {other:?}"),
        }
    }
    feed.publish(CaptureSequence::new(53), 1, CaptureChangeKind::Admitted);
    let next = receiver.recv().await.expect("next changed sequence");
    assert_eq!(
        next.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(53),
            expected_revision: None,
        },
        "queued revisions covered by the initial snapshot must be skipped"
    );
    let mut next_pending = pending_snapshot(53);
    next_pending.revision = 1;
    next.reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: next_pending,
            },
        ))))
        .expect("next capture reply receiver");

    feed.publish(
        CaptureSequence::new(52),
        6,
        CaptureChangeKind::RecordUpdated,
    );
    let changed = receiver.recv().await.expect("newer revision recheck");
    assert_eq!(
        changed.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(52),
            expected_revision: None,
        }
    );
    let mut terminal = completed_snapshot(52);
    terminal.revision = 6;
    changed
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: terminal,
            },
        ))))
        .expect("capture reply receiver");
    assert_matched(task.await.expect("wait task"), &instance, 52);
}

#[tokio::test]
async fn wait_for_capture_latest_materialization_collapses_queued_revisions() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed.clone(), 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "materialized-revision-watermark",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::ExchangeTerminal,
            Some(30_000),
        ),
        CancellationToken::new(),
    );
    reply_initial_search(&mut receiver, &instance, Vec::new()).await;
    for revision in 1..=3 {
        feed.publish(
            CaptureSequence::new(61),
            revision,
            CaptureChangeKind::RecordUpdated,
        );
    }

    let first = receiver
        .recv()
        .await
        .expect("first changed-sequence recheck");
    assert_eq!(
        first.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(61),
            expected_revision: None,
        }
    );
    let mut newest_pending = pending_snapshot(61);
    newest_pending.revision = 3;
    first
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: newest_pending,
            },
        ))))
        .expect("capture reply receiver");
    feed.publish(CaptureSequence::new(62), 1, CaptureChangeKind::Admitted);
    let next = receiver.recv().await.expect("next changed sequence");
    assert_eq!(
        next.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(62),
            expected_revision: None,
        },
        "materializing revision 3 must skip queued revisions 2 and 3"
    );
    let mut next_pending = pending_snapshot(62);
    next_pending.revision = 1;
    next.reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: next_pending,
            },
        ))))
        .expect("next capture reply receiver");

    feed.publish(
        CaptureSequence::new(61),
        4,
        CaptureChangeKind::RecordUpdated,
    );
    let second = receiver
        .recv()
        .await
        .expect("newer changed-sequence recheck");
    assert_eq!(
        second.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(61),
            expected_revision: None,
        }
    );
    let mut terminal = completed_snapshot(61);
    terminal.revision = 4;
    second
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: terminal,
            },
        ))))
        .expect("capture reply receiver");
    assert_matched(task.await.expect("wait task"), &instance, 61);
}

#[tokio::test(start_paused = true)]
async fn wait_for_capture_default_timeout_clamp_and_zero_validation_are_exact() {
    for (requested, normalized) in [(None, 30_000), (Some(u64::MAX), 300_000)] {
        let feed = CaptureChangeFeed::new();
        let (handler, mut receiver, _) = handler(feed, 2);
        let instance = scope();
        let task = spawn_wait(
            handler,
            "timeout-normalization",
            wait_request(
                CaptureQuery::default(),
                CaptureMilestone::RequestSeen,
                requested,
            ),
            CancellationToken::new(),
        );
        reply_initial_search(&mut receiver, &instance, Vec::new()).await;

        tokio::time::advance(Duration::from_millis(normalized - 1)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        let ControlResult::WaitForCapture { result, .. } =
            task.await.expect("wait task").expect("timeout success")
        else {
            panic!("expected wait result");
        };
        assert_eq!(
            result,
            WaitForCaptureResult {
                matched: false,
                capture: None,
            }
        );
    }

    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed, 1);
    let error = handler
        .handle(
            context("zero-timeout"),
            wait_request(
                CaptureQuery::default(),
                CaptureMilestone::RequestSeen,
                Some(0),
            ),
            CancellationToken::new(),
        )
        .await
        .expect_err("zero timeout is invalid");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn wait_for_capture_timeout_cancels_an_in_flight_runtime_batch() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, admission) = handler(feed, 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "timeout-cancels-runtime-batch",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(1_000),
        ),
        CancellationToken::new(),
    );

    let status = receiver.recv().await.expect("initial status command");
    assert_eq!(status.request, RuntimeRequest::GetStatus);
    status
        .reply
        .send(Ok(status_reply(&instance, 0)))
        .expect("status reply receiver");
    let batch = receiver.recv().await.expect("in-flight batch command");
    assert!(matches!(
        batch.request,
        RuntimeRequest::GetCaptureSearchBatch { .. }
    ));
    assert!(!batch.cancelled.is_cancelled());

    tokio::time::advance(Duration::from_secs(1)).await;
    let ControlResult::WaitForCapture { result, .. } =
        task.await.expect("wait task").expect("timeout success")
    else {
        panic!("expected wait result");
    };
    assert_eq!(
        result,
        WaitForCaptureResult {
            matched: false,
            capture: None,
        }
    );
    assert!(
        batch.cancelled.is_cancelled(),
        "a normal wait timeout must cancel queued runtime work"
    );
    assert_eq!(admission.available_permits_for_test(), 4);
}

#[tokio::test]
async fn wait_for_capture_cancelled_blocking_matcher_retains_permit_until_worker_exits() {
    let admission = Arc::new(CaptureSearchAdmission::new(1));
    let gate = Arc::new(BlockingGate::default());
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let cancelled = CancellationToken::new();
    let mut active = admission
        .admit(CaptureQuery::default(), cancelled.clone())
        .await
        .expect("initial search admission");
    let worker_gate = Arc::clone(&gate);
    let worker_cancelled = cancelled.clone();
    let worker = tokio::spawn(async move {
        active
            .run_blocking(worker_cancelled, move |_query| {
                started_tx.send(()).expect("started receiver");
                worker_gate.wait();
                Ok::<_, ControlError>(())
            })
            .await
    });
    started_rx.recv().await.expect("blocking matcher started");
    cancelled.cancel();
    let error = worker
        .await
        .expect("worker task")
        .expect_err("outer wait matcher cancelled");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert_eq!(
        admission.available_permits_for_test(),
        0,
        "detached blocking matcher retains its permit"
    );

    let second_admission = Arc::clone(&admission);
    let (attempted_tx, attempted_rx) = oneshot::channel();
    let second = tokio::spawn(async move {
        attempted_tx.send(()).expect("attempted receiver");
        second_admission
            .admit(CaptureQuery::default(), CancellationToken::new())
            .await
            .map(drop)
    });
    attempted_rx.await.expect("second admission attempted");
    assert!(!second.is_finished());

    gate.release();
    second
        .await
        .expect("second task")
        .expect("second admitted after worker exit");
    assert_eq!(admission.available_permits_for_test(), 1);
}

#[tokio::test]
async fn wait_for_capture_cancellation_interrupts_feed_sleep_without_leaking_search_permits() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, admission) = handler(feed, 1);
    let instance = scope();
    let cancelled = CancellationToken::new();
    let task = spawn_wait(
        handler,
        "cancel-sleeping-wait",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(300_000),
        ),
        cancelled.clone(),
    );
    reply_initial_search(&mut receiver, &instance, Vec::new()).await;
    tokio::task::yield_now().await;
    assert_eq!(admission.available_permits_for_test(), 4);

    cancelled.cancel();
    let error = task
        .await
        .expect("wait task")
        .expect_err("wait cancellation");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert_eq!(admission.available_permits_for_test(), 4);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn wait_for_capture_pending_removal_fails_but_unrelated_eviction_is_ignored() {
    for kind in [
        CaptureChangeKind::RetentionEviction,
        CaptureChangeKind::ExplicitDelete,
        CaptureChangeKind::Clear,
    ] {
        let feed = CaptureChangeFeed::new();
        let (handler, mut receiver, _) = handler(feed.clone(), 2);
        let instance = scope();
        let task = spawn_wait(
            handler,
            "pending-removal",
            wait_request(
                CaptureQuery::default(),
                CaptureMilestone::ExchangeTerminal,
                Some(30_000),
            ),
            CancellationToken::new(),
        );
        reply_initial_search(&mut receiver, &instance, vec![pending_snapshot(21)]).await;
        feed.publish(CaptureSequence::new(21), 5, kind);

        let error = task
            .await
            .expect("wait task")
            .expect_err("pending capture removal");
        assert_eq!(error.code, ControlErrorCode::CaptureNotFound);
        assert_eq!(error.details["capture_id"], json!(21));
        assert_eq!(error.details["capture_revision"], json!(5));
    }

    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed.clone(), 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "unrelated-removal",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::ExchangeTerminal,
            Some(30_000),
        ),
        CancellationToken::new(),
    );
    reply_initial_search(&mut receiver, &instance, vec![pending_snapshot(30)]).await;
    feed.publish(
        CaptureSequence::new(29),
        1,
        CaptureChangeKind::RetentionEviction,
    );
    feed.publish(
        CaptureSequence::new(30),
        2,
        CaptureChangeKind::RecordUpdated,
    );

    let command = receiver.recv().await.expect("pending update recheck");
    assert_eq!(
        command.request,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(30),
            expected_revision: None,
        }
    );
    command
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: instance.clone(),
                snapshot: completed_snapshot(30),
            },
        ))))
        .expect("capture receiver");
    assert_matched(task.await.expect("wait task"), &instance, 30);
}

#[tokio::test]
async fn wait_for_capture_materialization_loss_returns_retryable_capture_not_found() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed.clone(), 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "materialization-loss",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(30_000),
        ),
        CancellationToken::new(),
    );
    reply_initial_search(&mut receiver, &instance, Vec::new()).await;
    feed.publish(
        CaptureSequence::new(44),
        6,
        CaptureChangeKind::RecordUpdated,
    );

    let command = receiver.recv().await.expect("changed capture recheck");
    command
        .reply
        .send(Err(ControlError::new(
            ControlErrorCode::CaptureNotFound,
            "capture is no longer retained",
            false,
            json!({"capture_id": 44}),
        )))
        .expect("capture receiver");

    let error = task
        .await
        .expect("wait task")
        .expect_err("materialization uncertainty");
    assert_eq!(error.code, ControlErrorCode::CaptureNotFound);
    assert!(error.retryable);
    assert_eq!(error.details["capture_id"], json!(44));
    assert_eq!(error.details["capture_revision"], json!(6));
}

#[tokio::test]
async fn wait_for_capture_high_frequency_unrelated_changes_fail_as_an_explicit_bounded_gap() {
    let feed = CaptureChangeFeed::new();
    let (handler, mut receiver, _) = handler(feed.clone(), 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "bounded-gap",
        wait_request(
            CaptureQuery {
                method: Some("POST".to_owned()),
                ..CaptureQuery::default()
            },
            CaptureMilestone::ExchangeTerminal,
            Some(300_000),
        ),
        CancellationToken::new(),
    );
    reply_initial_search(&mut receiver, &instance, Vec::new()).await;

    for sequence in 1..=4_097 {
        feed.publish(
            CaptureSequence::new(sequence),
            0,
            CaptureChangeKind::Admitted,
        );
    }

    let error = task
        .await
        .expect("wait task")
        .expect_err("feed uncertainty must be explicit");
    assert_eq!(error.code, ControlErrorCode::ServiceUnavailable);
    assert!(error.retryable);
    assert_eq!(error.details["expected_epoch"], json!(1));
    assert_eq!(error.details["oldest_available_epoch"], json!(2));
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn wait_for_capture_publisher_feed_injected_into_control_context_wakes_real_wait_path() {
    let (capture_tx, mut capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let (handler, mut receiver, _) = handler(publisher.change_feed(), 2);
    let instance = scope();
    let task = spawn_wait(
        handler,
        "live-publisher-feed",
        wait_request(
            CaptureQuery::default(),
            CaptureMilestone::RequestSeen,
            Some(30_000),
        ),
        CancellationToken::new(),
    );

    loop {
        let command = receiver.recv().await.expect("initial wait command");
        match command.request {
            RuntimeRequest::GetStatus => command
                .reply
                .send(Ok(status_reply(&instance, 0)))
                .expect("status receiver"),
            RuntimeRequest::GetCaptureSearchBatch { .. } => {
                let headers = HeaderMap::new();
                let _handle = publisher
                    .try_start(RequestCaptureInput {
                        method: Method::GET,
                        original_uri: "https://publisher.example/live",
                        effective_uri: "https://publisher.example/live",
                        local_path: None,
                        headers: &headers,
                    })
                    .expect("publisher admission");
                let record = capture_rx.recv().await.expect("publisher record");
                record.bind_change_feed();
                command
                    .reply
                    .send(Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
                        snapshots: Vec::new(),
                        next_cursor: None,
                    })))
                    .expect("batch receiver");

                let changed = receiver.recv().await.expect("live publisher change");
                assert_eq!(
                    changed.request,
                    RuntimeRequest::GetCapture {
                        capture_id: record.sequence(),
                        expected_revision: None,
                    }
                );
                changed
                    .reply
                    .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
                        crate::control::CaptureSnapshotReply {
                            instance: instance.clone(),
                            snapshot: record.snapshot(CaptureSnapshotMode::MetadataOnly),
                        },
                    ))))
                    .expect("capture receiver");
                assert_matched(
                    task.await.expect("wait task"),
                    &instance,
                    record.sequence().value(),
                );
                return;
            }
            other => panic!("unexpected initial command: {other:?}"),
        }
    }
}
