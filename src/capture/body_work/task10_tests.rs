use super::{
    ACTIVE_BODY_WORK_LIMIT, BodyWorkAdmission, QUEUED_BODY_INPUT_LIMIT_BYTES,
    QUEUED_BODY_WORK_LIMIT,
};
use crate::control_rpc::protocol::ControlErrorCode;
use std::sync::{Arc, Condvar, Mutex};
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

async fn active(admission: &Arc<BodyWorkAdmission>) -> super::ActiveBodyWorkLease {
    admission
        .try_admit_mcp(1)
        .expect("queue body work")
        .acquire_active(deadline(), CancellationToken::new())
        .await
        .expect("active body work")
}

#[tokio::test]
async fn shared_tui_and_mcp_callers_observe_one_active_and_queue_budget() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = active(&admission).await;
    let second = active(&admission).await;
    assert_eq!(admission.test_snapshot().active, ACTIVE_BODY_WORK_LIMIT);

    let mut queued = Vec::new();
    for index in 0..QUEUED_BODY_WORK_LIMIT {
        let lease = if index % 2 == 0 {
            admission.try_admit_tui(1).expect("TUI queue slot")
        } else {
            admission.try_admit_mcp(1).expect("MCP queue slot")
        };
        queued.push(lease);
    }
    assert_eq!(admission.test_snapshot().queued, QUEUED_BODY_WORK_LIMIT);
    assert!(admission.try_admit_tui(1).is_none());
    let error = admission
        .try_admit_mcp(1)
        .expect_err("shared queue saturation");
    assert_eq!(error.code, ControlErrorCode::ResourceLimit);
    assert!(error.retryable);
    assert_eq!(error.details["maximum_queued_jobs"], QUEUED_BODY_WORK_LIMIT);

    drop((queued, first, second));
    assert_eq!(admission.test_snapshot().active, 0);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert_eq!(admission.test_snapshot().queued_bytes, 0);
}

#[test]
fn queued_input_charge_saturates_without_waiting_and_releases_on_drop() {
    assert_eq!(QUEUED_BODY_INPUT_LIMIT_BYTES, 32 * 1_024 * 1_024);
    let admission = BodyWorkAdmission::new();
    let full = admission
        .try_admit_mcp(QUEUED_BODY_INPUT_LIMIT_BYTES)
        .expect("full byte lease");
    assert_eq!(
        admission.test_snapshot().queued_bytes,
        QUEUED_BODY_INPUT_LIMIT_BYTES
    );
    assert!(admission.try_admit_tui(1).is_none());
    let error = admission
        .try_admit_mcp(1)
        .expect_err("queued byte saturation");
    assert_eq!(error.code, ControlErrorCode::ResourceLimit);
    assert_eq!(
        error.details,
        serde_json::json!({
            "requested_input_bytes": 1,
            "queued_input_bytes": QUEUED_BODY_INPUT_LIMIT_BYTES,
            "maximum_queued_input_bytes": QUEUED_BODY_INPUT_LIMIT_BYTES
        })
    );

    drop(full);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert_eq!(admission.test_snapshot().queued_bytes, 0);
}

#[tokio::test]
async fn queued_leases_are_released_only_after_active_capacity_is_owned() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = active(&admission).await;
    let second = active(&admission).await;
    let queued = admission.try_admit_mcp(4_096).expect("queued lease");
    assert_eq!(admission.test_snapshot().queued, 1);
    assert_eq!(admission.test_snapshot().queued_bytes, 4_096);

    let waiter = tokio::spawn({
        let cancelled = CancellationToken::new();
        async move { queued.acquire_active(deadline(), cancelled).await }
    });
    admission.test_wait_for_active_waiters(1).await;
    assert_eq!(admission.test_snapshot().queued, 1);
    assert_eq!(admission.test_snapshot().queued_bytes, 4_096);

    drop(first);
    let acquired = waiter.await.expect("waiter join").expect("active acquired");
    let snapshot = admission.test_snapshot();
    assert_eq!(snapshot.active, ACTIVE_BODY_WORK_LIMIT);
    assert_eq!(snapshot.queued, 0);
    assert_eq!(snapshot.queued_bytes, 0);
    drop((acquired, second));
}

#[tokio::test]
async fn cancellation_wins_while_waiting_for_active_capacity_and_releases_queue_charge() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let _first = active(&admission).await;
    let _second = active(&admission).await;
    let queued = admission.try_admit_mcp(8_192).expect("queued lease");
    let cancelled = CancellationToken::new();
    cancelled.cancel();

    let error = queued
        .acquire_active(deadline(), cancelled)
        .await
        .expect_err("cancelled queue wait");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert_eq!(admission.test_snapshot().queued_bytes, 0);
    assert_eq!(admission.test_snapshot().active, ACTIVE_BODY_WORK_LIMIT);
}

#[tokio::test(start_paused = true)]
async fn deadline_wins_while_waiting_for_active_capacity_and_releases_queue_charge() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let _first = active(&admission).await;
    let _second = active(&admission).await;
    let queued = admission.try_admit_mcp(8_192).expect("queued lease");
    let waiter = tokio::spawn(queued.acquire_active(
        Instant::now() + Duration::from_secs(3),
        CancellationToken::new(),
    ));

    tokio::time::advance(Duration::from_secs(3)).await;
    let error = waiter
        .await
        .expect("deadline waiter join")
        .expect_err("deadline queue wait");
    assert_eq!(error.code, ControlErrorCode::DeadlineExceeded);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert_eq!(admission.test_snapshot().queued_bytes, 0);
}

#[test]
fn cancellation_and_elapsed_deadline_win_before_saturated_queue_admission() {
    let admission = BodyWorkAdmission::new();
    let held = (0..QUEUED_BODY_WORK_LIMIT)
        .map(|_| admission.try_admit_mcp(0).expect("fill queue"))
        .collect::<Vec<_>>();
    let cancelled = CancellationToken::new();
    cancelled.cancel();

    let error = admission
        .try_admit_mcp_until(1, Instant::now() + Duration::from_secs(30), &cancelled)
        .expect_err("observable cancellation");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert!(error.retryable);

    let error = admission
        .try_admit_mcp_until(1, Instant::now(), &CancellationToken::new())
        .expect_err("elapsed deadline");
    assert_eq!(error.code, ControlErrorCode::DeadlineExceeded);
    drop(held);
}

#[tokio::test]
async fn revalidation_requeues_only_after_releasing_active_capacity() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let active = active(&admission).await;
    let queued_bytes = admission
        .try_admit_mcp(QUEUED_BODY_INPUT_LIMIT_BYTES)
        .expect("occupy queued bytes");
    assert_eq!(admission.test_snapshot().active, 1);

    let error = active
        .retry_mcp_after_revalidation(1, deadline(), &CancellationToken::new())
        .expect_err("changed charge cannot enter saturated queue");
    assert_eq!(error.code, ControlErrorCode::ResourceLimit);
    assert_eq!(
        admission.test_snapshot().active,
        0,
        "active must be released before the queued-byte retry"
    );
    assert_eq!(
        admission.test_snapshot().queued_bytes,
        QUEUED_BODY_INPUT_LIMIT_BYTES
    );
    drop(queued_bytes);
}

#[derive(Default)]
struct BlockingGate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl BlockingGate {
    fn wait(&self) {
        let mut open = self.open.lock().expect("gate");
        while !*open {
            open = self.changed.wait(open).expect("gate wait");
        }
    }

    fn release(&self) {
        *self.open.lock().expect("gate") = true;
        self.changed.notify_all();
    }
}

#[tokio::test]
async fn aborting_the_awaiter_does_not_release_active_capacity_before_blocking_worker_exit() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let active = active(&admission).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let exited = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(BlockingGate::default());
    let worker = tokio::task::spawn_blocking({
        let started = Arc::clone(&started);
        let exited = Arc::clone(&exited);
        let gate = Arc::clone(&gate);
        move || {
            let _active = active;
            started.notify_one();
            gate.wait();
            exited.notify_one();
        }
    });

    started.notified().await;
    worker.abort();
    assert_eq!(admission.test_snapshot().active, 1);
    gate.release();
    exited.notified().await;
    assert_eq!(admission.test_snapshot().active, 0);
}
