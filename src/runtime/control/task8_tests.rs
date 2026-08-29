#![cfg(unix)]

use super::{CaptureSearchAdmission, RuntimeGateway};
use crate::{
    app::{App, SettingsUiContext},
    capture::{
        CaptureRecord, CaptureRetentionPolicy, CaptureSequence, CapturedExchange,
    },
    control::{
        InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest,
        capture_query::CaptureSearchCursor,
    },
    control_rpc::protocol::{ControlErrorCode, InstanceScope},
    instance::InstanceIdentity,
    logging::LogRetentionPolicy,
    recording::RecordingState,
    runtime::event_loop::execute_control_request_for_test,
    settings::{AppSettings, ConfigMode, PersistenceMode, SettingsSession},
};
use hyper::Method;
use std::sync::{Arc, Condvar, Mutex};
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

#[test]
fn recording_setter_reports_previous_and_current_and_never_changes_launch_settings() {
    let (identity, mut app, settings) = runtime_fixture(10);
    assert!(!settings.snapshot().recording.start_record_on_launch);

    for (enabled, expected_previous, expected_current) in
        [(true, false, true), (true, true, true), (false, true, false)]
    {
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
    let RuntimeReply::CaptureDetail(detail) = current else {
        panic!("expected capture detail")
    };
    assert_eq!(detail.capture_sequence, CaptureSequence::new(7));
    assert_eq!(detail.capture_revision, revision);

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
            admission
                .run_blocking(CancellationToken::new(), move || {
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
    let fifth_started_tx = started_tx.clone();
    let fifth = tokio::spawn(async move {
        attempted_tx.send(()).expect("attempted receiver");
        fifth_admission
            .run_blocking(fifth_cancelled, move || {
                fifth_started_tx.send(5).expect("started receiver");
                Ok::<_, crate::control_rpc::protocol::ControlError>(())
            })
            .await
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
        first_admission
            .run_blocking(CancellationToken::new(), move || {
                first_started.send(1).expect("started receiver");
                first_gate.wait();
                Ok::<_, crate::control_rpc::protocol::ControlError>(())
            })
            .await
    });
    assert_eq!(started_rx.recv().await, Some(1));

    first.abort();
    assert!(first.await.expect_err("outer future aborted").is_cancelled());
    assert_eq!(admission.available_permits_for_test(), 0);

    let second_admission = Arc::clone(&admission);
    let second_started = started_tx.clone();
    let (attempted_tx, attempted_rx) = oneshot::channel();
    let second = tokio::spawn(async move {
        attempted_tx.send(()).expect("attempted receiver");
        second_admission
            .run_blocking(CancellationToken::new(), move || {
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
async fn a_blocked_maximum_header_matcher_does_not_block_an_unrelated_runtime_status_command() {
    let admission = Arc::new(CaptureSearchAdmission::new(4));
    let gate = Arc::new(BlockingGate::default());
    let (matcher_started_tx, matcher_started_rx) = oneshot::channel();
    let search_admission = Arc::clone(&admission);
    let search_gate = Arc::clone(&gate);
    let search = tokio::spawn(async move {
        search_admission
            .run_blocking(CancellationToken::new(), move || {
                matcher_started_tx.send(()).expect("matcher receiver");
                search_gate.wait();
                Ok::<_, crate::control_rpc::protocol::ControlError>(())
            })
            .await
    });
    matcher_started_rx.await.expect("blocking matcher started");

    let (identity, _, _) = runtime_fixture(1);
    let expected_run_id = identity.run_id().clone();
    let status_reply = RuntimeReply::Instance(InstanceRuntimeSnapshot {
        instance: InstanceScope {
            proxy_endpoint: identity.proxy_endpoint(),
            run_id: expected_run_id.clone(),
        },
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        recording_enabled: false,
        retained_capture_count: 0,
        settings_revision: 0,
    });
    let (client, mut receiver) = RuntimeGateway::new(1);
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
