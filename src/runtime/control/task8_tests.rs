#![cfg(unix)]

use super::{
    CaptureSearchAdmission, DetailMaterializationAdmission, RuntimeControlHandler, RuntimeGateway,
};
use crate::{
    app::{App, SettingsUiContext},
    capture::{CaptureRecord, CaptureRetentionPolicy, CaptureSequence, CapturedExchange},
    control::{
        InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest,
        capture_query::{
            CaptureHeaderFilter, CaptureQuery, CaptureSearchBatch, CaptureSearchCursor,
            CaptureStatusFilter, CaptureTextFilter,
        },
    },
    control_rpc::{
        protocol::{
            ControlErrorCode, ControlOperation, ControlResult, DeclaredClient, InstanceScope,
        },
        server::{ControlCallContext, ControlRpcHandler},
    },
    instance::InstanceIdentity,
    logging::LogRetentionPolicy,
    recording::RecordingState,
    runtime::event_loop::execute_control_request_for_test,
    settings::{AppSettings, ConfigMode, PersistenceMode, SettingsSession},
};
use hyper::Method;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

fn completed_capture(sequence: u64) -> Arc<CaptureRecord> {
    CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: Method::GET,
        uri: format!("https://example.test/{sequence}"),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![("X-Sequence".to_owned(), sequence.to_string())],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    })
}

fn completed_capture_with_method(sequence: u64, method: Method) -> Arc<CaptureRecord> {
    CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method,
        uri: format!("https://example.test/{sequence}"),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    })
}

fn runtime_fixture(max_records: usize) -> (InstanceIdentity, App, SettingsSession) {
    let mut launch = AppSettings::default();
    launch.recording.start_record_on_launch = false;
    let settings = SettingsSession::temporary(launch.clone());
    let app = App::with_runtime_policies(
        launch,
        RecordingState::new(false),
        LogRetentionPolicy::default(),
        CaptureRetentionPolicy {
            max_records,
            max_bytes: usize::MAX,
        },
        SettingsUiContext::default(),
    );
    let identity =
        InstanceIdentity::new("127.0.0.1:19028".parse().expect("endpoint")).expect("identity");
    (identity, app, settings)
}

fn control_context(request_id: impl Into<String>) -> ControlCallContext {
    ControlCallContext {
        request_id: request_id.into(),
        declared_client: DeclaredClient {
            name: "task-8-test".to_owned(),
            version: "1".to_owned(),
        },
        deadline: Instant::now() + Duration::from_secs(30),
    }
}

#[test]
fn runtime_capture_batches_are_newest_first_and_never_exceed_thirty_two_snapshots() {
    let (identity, mut app, settings) = runtime_fixture(100);
    for sequence in 1..=40 {
        app.add_capture(completed_capture(sequence));
    }

    let reply = execute_control_request_for_test(
        &identity,
        &mut app,
        &settings,
        RuntimeRequest::GetCaptureSearchBatch {
            cursor: None,
            max_rows: 100,
        },
    )
    .expect("capture batch");
    let RuntimeReply::CaptureSearchBatch(batch) = reply else {
        panic!("expected capture search batch")
    };

    assert_eq!(batch.snapshots.len(), 32);
    assert_eq!(
        batch
            .snapshots
            .iter()
            .map(|capture| capture.sequence.value())
            .collect::<Vec<_>>(),
        (9..=40).rev().collect::<Vec<_>>()
    );
    assert_eq!(
        batch.next_cursor,
        Some(CaptureSearchCursor::new(CaptureSequence::new(8)))
    );
}

#[test]
fn runtime_capture_batch_respects_an_inclusive_older_cursor() {
    let (identity, mut app, settings) = runtime_fixture(100);
    for sequence in 1..=40 {
        app.add_capture(completed_capture(sequence));
    }

    let reply = execute_control_request_for_test(
        &identity,
        &mut app,
        &settings,
        RuntimeRequest::GetCaptureSearchBatch {
            cursor: Some(CaptureSearchCursor::new(CaptureSequence::new(30))),
            max_rows: 10,
        },
    )
    .expect("capture batch");
    let RuntimeReply::CaptureSearchBatch(batch) = reply else {
        panic!("expected capture search batch")
    };

    assert_eq!(
        batch
            .snapshots
            .iter()
            .map(|capture| capture.sequence.value())
            .collect::<Vec<_>>(),
        (21..=30).rev().collect::<Vec<_>>()
    );
    assert_eq!(
        batch.next_cursor,
        Some(CaptureSearchCursor::new(CaptureSequence::new(20)))
    );
}

#[tokio::test]
async fn accumulated_multi_batch_page_omits_a_row_only_against_remaining_capacity_and_resumes_it() {
    let (identity, mut app, settings) = runtime_fixture(100);
    let omitted_method =
        Method::from_bytes(&vec![b'Z'; 2 * 1024 * 1024]).expect("large extension method");
    app.add_capture(completed_capture_with_method(1, omitted_method));
    for sequence in 2..=33 {
        let method = Method::from_bytes(&vec![b'M'; 180 * 1024]).expect("medium extension method");
        app.add_capture(completed_capture_with_method(sequence, method));
    }

    let scope = InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    };
    let (client, mut receiver) = RuntimeGateway::channel(4);
    let handler = RuntimeControlHandler::new(client);
    let first_handler = handler.clone();
    let first = tokio::spawn(async move {
        first_handler
            .handle(
                control_context("multi-batch-first"),
                ControlOperation::SearchCaptures {
                    query: Box::new(CaptureQuery::default()),
                    cursor: None,
                    limit: Some(100),
                },
                CancellationToken::new(),
            )
            .await
    });
    for expected in [
        RuntimeRequest::GetStatus,
        RuntimeRequest::GetCaptureSearchBatch {
            cursor: None,
            max_rows: 32,
        },
        RuntimeRequest::GetCaptureSearchBatch {
            cursor: Some(CaptureSearchCursor::new(CaptureSequence::new(1))),
            max_rows: 32,
        },
    ] {
        let command = receiver.recv().await.expect("first search runtime command");
        assert_eq!(command.request, expected);
        let reply =
            execute_control_request_for_test(&identity, &mut app, &settings, command.request);
        command
            .reply
            .send(reply)
            .expect("first search reply receiver");
    }
    let ControlResult::SearchCaptures {
        instance,
        captures,
        next_cursor,
    } = first.await.expect("first search task").expect("first page")
    else {
        panic!("expected first capture search page")
    };
    assert_eq!(instance, scope);
    assert_eq!(
        captures
            .iter()
            .map(|capture| capture.capture_sequence.value())
            .collect::<Vec<_>>(),
        (2..=33).rev().collect::<Vec<_>>()
    );
    assert_eq!(
        next_cursor,
        Some(CaptureSearchCursor::new(CaptureSequence::new(1)))
    );

    let second = tokio::spawn(async move {
        handler
            .handle(
                control_context("multi-batch-second"),
                ControlOperation::SearchCaptures {
                    query: Box::new(CaptureQuery::default()),
                    cursor: Some(CaptureSearchCursor::new(CaptureSequence::new(1))),
                    limit: Some(100),
                },
                CancellationToken::new(),
            )
            .await
    });
    for expected in [
        RuntimeRequest::GetStatus,
        RuntimeRequest::GetCaptureSearchBatch {
            cursor: Some(CaptureSearchCursor::new(CaptureSequence::new(1))),
            max_rows: 32,
        },
    ] {
        let command = receiver
            .recv()
            .await
            .expect("second search runtime command");
        assert_eq!(command.request, expected);
        let reply =
            execute_control_request_for_test(&identity, &mut app, &settings, command.request);
        command
            .reply
            .send(reply)
            .expect("second search reply receiver");
    }
    let ControlResult::SearchCaptures {
        instance,
        captures,
        next_cursor,
    } = second
        .await
        .expect("second search task")
        .expect("second page")
    else {
        panic!("expected second capture search page")
    };
    assert_eq!(instance, scope);
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].capture_sequence, CaptureSequence::new(1));
    assert_eq!(next_cursor, None);
}

#[test]
fn recording_setter_reports_previous_and_current_and_never_changes_launch_settings() {
    let (identity, mut app, settings) = runtime_fixture(10);
    assert!(!settings.snapshot().recording.start_record_on_launch);

    for (enabled, expected_previous, expected_current) in [
        (true, false, true),
        (true, true, true),
        (false, true, false),
    ] {
        let reply = execute_control_request_for_test(
            &identity,
            &mut app,
            &settings,
            RuntimeRequest::SetRecordingEnabled { enabled },
        )
        .expect("recording update");
        let RuntimeReply::RecordingUpdated(result) = reply else {
            panic!("expected recording update")
        };
        assert_eq!(
            result.instance,
            InstanceScope {
                proxy_endpoint: identity.proxy_endpoint(),
                run_id: identity.run_id().clone(),
            }
        );
        assert_eq!(result.previous, expected_previous);
        assert_eq!(result.current, expected_current);
        assert_eq!(app.is_recording(), expected_current);
        assert!(
            !settings.snapshot().recording.start_record_on_launch,
            "live recording mutation must not rewrite the launch setting"
        );
    }
}

#[test]
fn capture_detail_uses_current_revision_and_distinguishes_conflict_not_found_and_eviction() {
    let (identity, mut app, settings) = runtime_fixture(1);
    let retained = completed_capture(7);
    let revision = retained.revision();
    app.add_capture(Arc::clone(&retained));

    let current = execute_control_request_for_test(
        &identity,
        &mut app,
        &settings,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(7),
            expected_revision: Some(revision),
        },
    )
    .expect("current detail");
    let RuntimeReply::CaptureSnapshot(capture) = current else {
        panic!("expected capture snapshot")
    };
    assert_eq!(capture.instance.run_id, identity.run_id().clone());
    assert_eq!(capture.snapshot.sequence, CaptureSequence::new(7));
    assert_eq!(capture.snapshot.revision, revision);

    let conflict = execute_control_request_for_test(
        &identity,
        &mut app,
        &settings,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(7),
            expected_revision: Some(revision.saturating_add(1)),
        },
    )
    .expect_err("revision conflict");
    assert_eq!(conflict.code, ControlErrorCode::CaptureRevisionConflict);
    assert_eq!(conflict.details["current_revision"], revision);

    let missing = execute_control_request_for_test(
        &identity,
        &mut app,
        &settings,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(99),
            expected_revision: None,
        },
    )
    .expect_err("missing capture");
    assert_eq!(missing.code, ControlErrorCode::CaptureNotFound);

    app.add_capture(completed_capture(8));
    let evicted = execute_control_request_for_test(
        &identity,
        &mut app,
        &settings,
        RuntimeRequest::GetCapture {
            capture_id: CaptureSequence::new(7),
            expected_revision: None,
        },
    )
    .expect_err("retention eviction");
    assert_eq!(evicted.code, ControlErrorCode::CaptureNotFound);
}
#[tokio::test]
async fn recording_mutation_returns_same_turn_identity_without_a_follow_up_status_request() {
    let (identity, _, _) = runtime_fixture(1);
    let scope = InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    };
    let (client, mut receiver) = RuntimeGateway::channel(4);
    let handler = RuntimeControlHandler::new(client);
    let call = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .handle(
                    control_context("recording-same-turn"),
                    ControlOperation::SetRecordingEnabled { enabled: true },
                    CancellationToken::new(),
                )
                .await
        }
    });

    let command = receiver.recv().await.expect("recording command");
    assert_eq!(
        command.request,
        RuntimeRequest::SetRecordingEnabled { enabled: true }
    );
    command
        .reply
        .send(Ok(RuntimeReply::RecordingUpdated(
            crate::control::RecordingUpdate {
                instance: scope.clone(),
                previous: false,
                current: true,
            },
        )))
        .expect("recording reply receiver");

    let ControlResult::SetRecordingEnabled { instance, .. } =
        call.await.expect("handler task").expect("recording result")
    else {
        panic!("expected recording result")
    };
    assert_eq!(instance, scope);
    assert!(
        receiver.try_recv().is_err(),
        "recording mutation must not issue a racy follow-up status request"
    );
}

#[tokio::test]
async fn admitted_recording_mutation_observes_its_authoritative_reply_after_cancellation() {
    let (identity, _, _) = runtime_fixture(1);
    let scope = InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    };
    let (client, mut receiver) = RuntimeGateway::channel(1);
    let handler = RuntimeControlHandler::new(client);
    let cancelled = CancellationToken::new();
    let mut call = tokio::spawn({
        let cancelled = cancelled.clone();
        async move {
            handler
                .handle(
                    control_context("recording-terminal-delivery"),
                    ControlOperation::SetRecordingEnabled { enabled: true },
                    cancelled,
                )
                .await
        }
    });

    let command = receiver.recv().await.expect("recording command");
    cancelled.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut call)
            .await
            .is_err(),
        "an admitted recording mutation must wait for its authoritative reply"
    );
    command
        .reply
        .send(Ok(RuntimeReply::RecordingUpdated(
            crate::control::RecordingUpdate {
                instance: scope,
                previous: false,
                current: true,
            },
        )))
        .expect("recording reply receiver");

    assert!(matches!(
        call.await.expect("handler task"),
        Ok(ControlResult::SetRecordingEnabled { current: true, .. })
    ));
}

#[tokio::test]
async fn private_handler_rejects_invalid_search_semantics_before_runtime_dispatch() {
    let (client, mut receiver) = RuntimeGateway::channel(1);
    let handler = RuntimeControlHandler::new(client);
    let cases = [
        (
            CaptureQuery {
                original_url: Some(CaptureTextFilter::Glob("[unterminated".to_owned())),
                ..CaptureQuery::default()
            },
            Some(20),
        ),
        (
            CaptureQuery {
                status: Some(CaptureStatusFilter::default()),
                ..CaptureQuery::default()
            },
            Some(20),
        ),
        (
            CaptureQuery {
                started_at_min: Some("not-a-time".to_owned()),
                ..CaptureQuery::default()
            },
            Some(20),
        ),
        (
            CaptureQuery {
                sequence_min: Some(9),
                sequence_max: Some(8),
                ..CaptureQuery::default()
            },
            Some(20),
        ),
        (CaptureQuery::default(), Some(0)),
        (CaptureQuery::default(), Some(101)),
    ];

    for (index, (query, limit)) in cases.into_iter().enumerate() {
        let error = handler
            .handle(
                control_context(format!("invalid-search-{index}")),
                ControlOperation::SearchCaptures {
                    query: Box::new(query),
                    cursor: None,
                    limit,
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("invalid query");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }
    assert!(
        receiver.try_recv().is_err(),
        "invalid searches must not reach AppRuntime"
    );
}

#[tokio::test]
async fn private_detail_uses_the_identity_returned_with_its_single_runtime_snapshot() {
    let (identity, _, _) = runtime_fixture(1);
    let scope = InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    };
    let snapshot = completed_capture(7).snapshot(crate::capture::CaptureSnapshotMode::MetadataOnly);
    let (client, mut receiver) = RuntimeGateway::channel(2);
    let handler = RuntimeControlHandler::new(client);
    let call = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .handle(
                    control_context("detail-same-turn"),
                    ControlOperation::GetCapture {
                        capture_id: CaptureSequence::new(7),
                        expected_revision: Some(0),
                    },
                    CancellationToken::new(),
                )
                .await
        }
    });

    let command = receiver.recv().await.expect("detail command");
    assert!(matches!(command.request, RuntimeRequest::GetCapture { .. }));
    command
        .reply
        .send(Ok(RuntimeReply::CaptureSnapshot(Box::new(
            crate::control::CaptureSnapshotReply {
                instance: scope.clone(),
                snapshot,
            },
        ))))
        .expect("detail reply receiver");

    let ControlResult::GetCapture { instance, capture } =
        call.await.expect("detail task").expect("detail result")
    else {
        panic!("expected detail result")
    };
    assert_eq!(instance, scope);
    assert_eq!(capture.capture_sequence, CaptureSequence::new(7));
    assert!(
        receiver.try_recv().is_err(),
        "detail must not issue a racy status request"
    );
}

#[derive(Default)]
struct BlockingGate {
    released: Mutex<bool>,
    changed: Condvar,
}

impl BlockingGate {
    fn wait(&self) {
        let mut released = self.released.lock().expect("gate");
        while !*released {
            released = self.changed.wait(released).expect("gate wait");
        }
    }

    fn release(&self) {
        *self.released.lock().expect("gate") = true;
        self.changed.notify_all();
    }
}

#[tokio::test]
async fn detail_materialization_is_bounded_and_keeps_permits_until_detached_workers_exit() {
    let admission = Arc::new(DetailMaterializationAdmission::new(4));
    let gate = Arc::new(BlockingGate::default());
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let mut active = Vec::new();
    for index in 0..4 {
        let admission = Arc::clone(&admission);
        let gate = Arc::clone(&gate);
        let started = started_tx.clone();
        let cancelled = CancellationToken::new();
        let worker_cancelled = cancelled.clone();
        let task = tokio::spawn(async move {
            admission
                .run_blocking(worker_cancelled, move || {
                    started.send(index).expect("started receiver");
                    gate.wait();
                })
                .await
        });
        active.push((cancelled, task));
    }
    for _ in 0..4 {
        started_rx.recv().await.expect("four active details");
    }
    assert_eq!(admission.available_permits_for_test(), 0);

    let (detached_cancelled, detached) = active.remove(0);
    detached_cancelled.cancel();
    let error = detached
        .await
        .expect("cancelled detail task")
        .expect_err("outer detail future cancelled");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert_eq!(
        admission.available_permits_for_test(),
        0,
        "the detached blocking worker must retain its permit"
    );

    let fifth_admission = Arc::clone(&admission);
    let fifth_gate = Arc::clone(&gate);
    let fifth_started = started_tx.clone();
    let (fifth_attempted_tx, fifth_attempted_rx) = oneshot::channel();
    let fifth = tokio::spawn(async move {
        fifth_attempted_tx.send(()).expect("attempted receiver");
        fifth_admission
            .run_blocking(CancellationToken::new(), move || {
                fifth_started.send(4).expect("started receiver");
                fifth_gate.wait();
            })
            .await
    });
    fifth_attempted_rx.await.expect("fifth attempted admission");
    assert!(
        started_rx.try_recv().is_err(),
        "a fifth detail worker must wait for admission"
    );

    gate.release();
    assert_eq!(started_rx.recv().await, Some(4));
    fifth
        .await
        .expect("fifth detail task")
        .expect("fifth detail");
    for (_, task) in active {
        task.await.expect("detail task").expect("detail result");
    }
}

#[tokio::test]
async fn at_most_four_capture_searches_are_active_and_a_fifth_cancels_while_waiting() {
    let admission = Arc::new(CaptureSearchAdmission::new(4));
    let gate = Arc::new(BlockingGate::default());
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let mut active = Vec::new();

    for index in 0..4 {
        let admission = Arc::clone(&admission);
        let gate = Arc::clone(&gate);
        let started_tx = started_tx.clone();
        active.push(tokio::spawn(async move {
            let cancelled = CancellationToken::new();
            let mut search = admission
                .admit(CaptureQuery::default(), cancelled.clone())
                .await?;
            search
                .run_blocking(cancelled, move |_query| {
                    started_tx.send(index).expect("started receiver");
                    gate.wait();
                    Ok::<_, crate::control_rpc::protocol::ControlError>(())
                })
                .await
        }));
    }
    for _ in 0..4 {
        started_rx.recv().await.expect("four active searches");
    }
    assert_eq!(admission.available_permits_for_test(), 0);

    let fifth_cancelled = CancellationToken::new();
    let cancellation = fifth_cancelled.clone();
    let (attempted_tx, attempted_rx) = oneshot::channel();
    let fifth_admission = Arc::clone(&admission);
    let fifth = tokio::spawn(async move {
        attempted_tx.send(()).expect("attempted receiver");
        fifth_admission
            .admit(CaptureQuery::default(), fifth_cancelled)
            .await
            .map(drop)
    });
    attempted_rx.await.expect("fifth attempted admission");
    assert!(started_rx.try_recv().is_err());
    cancellation.cancel();
    let error = fifth
        .await
        .expect("fifth task")
        .expect_err("fifth cancelled");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert_eq!(admission.available_permits_for_test(), 0);

    gate.release();
    for task in active {
        task.await.expect("active task").expect("active result");
    }
}

#[tokio::test]
async fn dropping_outer_search_keeps_its_permit_until_the_blocking_matcher_exits() {
    let admission = Arc::new(CaptureSearchAdmission::new(1));
    let gate = Arc::new(BlockingGate::default());
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let first_admission = Arc::clone(&admission);
    let first_gate = Arc::clone(&gate);
    let first_started = started_tx.clone();
    let first = tokio::spawn(async move {
        let cancelled = CancellationToken::new();
        let mut search = first_admission
            .admit(CaptureQuery::default(), cancelled.clone())
            .await?;
        search
            .run_blocking(cancelled, move |_query| {
                first_started.send(1).expect("started receiver");
                first_gate.wait();
                Ok::<_, crate::control_rpc::protocol::ControlError>(())
            })
            .await
    });
    assert_eq!(started_rx.recv().await, Some(1));

    first.abort();
    assert!(
        first
            .await
            .expect_err("outer future aborted")
            .is_cancelled()
    );
    assert_eq!(admission.available_permits_for_test(), 0);

    let second_admission = Arc::clone(&admission);
    let second_started = started_tx.clone();
    let (attempted_tx, attempted_rx) = oneshot::channel();
    let second = tokio::spawn(async move {
        attempted_tx.send(()).expect("attempted receiver");
        let cancelled = CancellationToken::new();
        let mut search = second_admission
            .admit(CaptureQuery::default(), cancelled.clone())
            .await?;
        search
            .run_blocking(cancelled, move |_query| {
                second_started.send(2).expect("started receiver");
                Ok::<_, crate::control_rpc::protocol::ControlError>(())
            })
            .await
    });
    attempted_rx.await.expect("second attempted admission");
    assert!(started_rx.try_recv().is_err());

    gate.release();
    assert_eq!(started_rx.recv().await, Some(2));
    second.await.expect("second task").expect("second result");
}

#[tokio::test]
async fn a_blocked_real_header_matcher_does_not_block_an_unrelated_runtime_status_command() {
    let admission = Arc::new(CaptureSearchAdmission::new(4));
    let gate = Arc::new(BlockingGate::default());
    let (matcher_started_tx, matcher_started_rx) = oneshot::channel();
    let search_admission = Arc::clone(&admission);
    let search_gate = Arc::clone(&gate);
    let large = CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(99),
        method: Method::GET,
        uri: "https://example.test/large".to_owned(),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![("X-Large".to_owned(), "ß".repeat(128 * 1024))],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    })
    .snapshot(crate::capture::CaptureSnapshotMode::MetadataOnly);
    let query = CaptureQuery {
        header: Some(CaptureHeaderFilter {
            name: "x-large".to_owned(),
            value: Some("not-present".to_owned()),
        }),
        ..CaptureQuery::default()
    };
    let search = tokio::spawn(async move {
        let cancelled = CancellationToken::new();
        let matcher_cancelled = cancelled.clone();
        let mut active = search_admission.admit(query, cancelled.clone()).await?;
        active
            .run_blocking(cancelled, move |query| {
                let mut scanned = 0;
                let mut started = Some(matcher_started_tx);
                query.matches_with_scan_hook(&large, &matcher_cancelled, |bytes| {
                    scanned += bytes;
                    if scanned > "X-Large".len()
                        && let Some(started) = started.take()
                    {
                        started.send(()).expect("matcher receiver");
                        search_gate.wait();
                    }
                })
            })
            .await
    });
    matcher_started_rx.await.expect("blocking matcher started");

    let (identity, _, _) = runtime_fixture(1);
    let expected_run_id = identity.run_id().clone();
    let status_reply = RuntimeReply::Instance(Box::new(InstanceRuntimeSnapshot {
        instance: InstanceScope {
            proxy_endpoint: identity.proxy_endpoint(),
            run_id: expected_run_id.clone(),
        },
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        recording_enabled: false,
        retained_capture_count: 0,
        settings_revision: 0,
        mapping: Default::default(),
        capture_store: Default::default(),
        metrics: Default::default(),
    }));
    let (client, mut receiver) = RuntimeGateway::channel(1);
    let status = tokio::spawn(async move {
        client
            .request(RuntimeRequest::GetStatus, CancellationToken::new())
            .await
    });
    let command = receiver.recv().await.expect("status reaches runtime");
    assert_eq!(command.request, RuntimeRequest::GetStatus);
    command
        .reply
        .send(Ok(status_reply))
        .expect("status receiver");
    assert_eq!(
        status
            .await
            .expect("status task")
            .expect("status reply")
            .instance()
            .instance
            .run_id,
        expected_run_id
    );

    gate.release();
    search.await.expect("search task").expect("search result");
}

#[tokio::test]
async fn real_search_handler_admits_only_four_calls_and_cancels_the_fifth_before_runtime_work() {
    let (identity, _, _) = runtime_fixture(1);
    let scope = InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    };
    let status = RuntimeReply::Instance(Box::new(InstanceRuntimeSnapshot {
        instance: scope,
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        recording_enabled: false,
        retained_capture_count: 0,
        settings_revision: 0,
        mapping: Default::default(),
        capture_store: Default::default(),
        metrics: Default::default(),
    }));
    let (client, mut receiver) = RuntimeGateway::channel(32);
    let handler = RuntimeControlHandler::new(client);
    let mut calls = Vec::new();
    for index in 0..5 {
        let cancelled = CancellationToken::new();
        let task = tokio::spawn({
            let handler = handler.clone();
            let task_cancelled = cancelled.clone();
            async move {
                handler
                    .handle(
                        control_context(format!("search-{index}")),
                        ControlOperation::SearchCaptures {
                            query: Box::new(CaptureQuery::default()),
                            cursor: None,
                            limit: Some(20),
                        },
                        task_cancelled,
                    )
                    .await
            }
        });
        calls.push((cancelled, task));
    }

    let mut batches = Vec::new();
    while batches.len() < 4 {
        let command = receiver.recv().await.expect("admitted search work");
        match command.request {
            RuntimeRequest::GetStatus => {
                command
                    .reply
                    .send(Ok(status.clone()))
                    .expect("status receiver");
            }
            RuntimeRequest::GetCaptureSearchBatch { .. } => batches.push(command),
            other => panic!("unexpected runtime request: {other:?}"),
        }
    }
    assert!(
        receiver.try_recv().is_err(),
        "the fifth search must wait before sending runtime work"
    );

    calls[4].0.cancel();
    let (_, fifth) = calls.pop().expect("fifth call");
    let error = fifth
        .await
        .expect("fifth task")
        .expect_err("fifth cancelled while waiting");
    assert_eq!(error.code, ControlErrorCode::Cancelled);

    for command in batches {
        command
            .reply
            .send(Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
                snapshots: Vec::new(),
                next_cursor: None,
            })))
            .expect("batch receiver");
    }
    for (_, task) in calls {
        let ControlResult::SearchCaptures { captures, .. } =
            task.await.expect("search task").expect("search result")
        else {
            panic!("expected search result")
        };
        assert!(captures.is_empty());
    }
}
