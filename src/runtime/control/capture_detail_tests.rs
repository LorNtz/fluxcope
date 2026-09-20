#![cfg(unix)]

use super::{
    DetailMaterializationAdmission, RuntimeControlHandler, RuntimeGateway,
    capture_test_support::{BlockingGate, completed_capture, control_context, runtime_fixture},
};
use crate::{
    capture::CaptureSequence,
    control::{RuntimeReply, RuntimeRequest},
    control_rpc::{
        protocol::{ControlErrorCode, ControlOperation, ControlResult, InstanceScope},
        server::ControlRpcHandler,
    },
    runtime::event_loop::execute_control_request_for_test,
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

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
