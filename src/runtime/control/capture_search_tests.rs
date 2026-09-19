#![cfg(unix)]

use super::{
    CaptureSearchAdmission, RuntimeControlHandler, RuntimeGateway,
    capture_test_support::{BlockingGate, completed_capture, control_context, runtime_fixture},
};
use crate::{
    capture::{CaptureRecord, CaptureSequence, CapturedExchange},
    control::{
        InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest,
        capture_query::{
            CaptureHeaderFilter, CaptureQuery, CaptureSearchBatch, CaptureSearchCursor,
            CaptureStatusFilter, CaptureTextFilter,
        },
    },
    control_rpc::{
        protocol::{ControlErrorCode, ControlOperation, ControlResult, InstanceScope},
        server::ControlRpcHandler,
    },
    runtime::event_loop::execute_control_request_for_test,
    settings::{ConfigMode, PersistenceMode},
};
use hyper::Method;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

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
