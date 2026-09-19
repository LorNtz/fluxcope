#![cfg(unix)]

use std::sync::{Arc, Condvar, Mutex, mpsc};

use tokio_util::sync::CancellationToken;

use super::{
    SettingsTransactionOrigin,
    test_support::{SettingsRuntimeFixture, create_preset},
};
use crate::{
    control_rpc::protocol::ControlErrorCode,
    settings::{ConfigMode, PersistenceMode, ProxySettings},
};

#[tokio::test]
async fn mapping_read_validate_and_explain_leave_all_authoritative_state_unchanged() {
    let fixture = SettingsRuntimeFixture::temporary_with_remote_presets().await;
    let before = fixture.snapshot();

    let read = fixture
        .get_mapping_settings()
        .await
        .expect("mapping snapshot");
    assert_eq!(read.revision, before.revision);
    assert_eq!(read.config_mode, ConfigMode::Temporary);
    assert_eq!(read.persistence, PersistenceMode::Ephemeral);
    let proxy = before.settings.proxy.clone().unwrap_or_default();

    let validation = fixture
        .validate_mapping_settings(proxy.clone())
        .await
        .expect("validation");
    assert!(validation.diagnostics.is_empty());
    let current = fixture
        .explain_mapping("https://api.example.com/v1", None)
        .await
        .expect("current explanation");
    let proposed = fixture
        .explain_mapping("https://api.example.com/v1", Some(proxy))
        .await
        .expect("proposed explanation");
    assert_eq!(current.effective_url, proposed.effective_url);

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.external_write_count(), 0);
    assert_eq!(fixture.policy_swap_count(), 0);
}

#[tokio::test]
async fn mapping_settings_snapshots_hold_worker_admission_until_dropped() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    let first = fixture
        .client()
        .get_mapping_settings(Default::default(), CancellationToken::new())
        .await
        .expect("first mapping snapshot");
    let second = fixture
        .client()
        .get_mapping_settings(Default::default(), CancellationToken::new())
        .await
        .expect("second mapping snapshot");
    let turns_before_blocked_read = fixture.runtime_turn_count();

    let blocked = tokio::time::timeout(
        std::time::Duration::from_millis(25),
        fixture
            .client()
            .validate_mapping_settings(ProxySettings::default(), CancellationToken::new()),
    )
    .await;
    assert!(
        blocked.is_err(),
        "two undelivered mapping snapshots must retain both worker permits"
    );
    assert_eq!(
        fixture.runtime_turn_count(),
        turns_before_blocked_read,
        "blocked work must not fetch another runtime snapshot"
    );

    drop(first);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture
            .client()
            .validate_mapping_settings(ProxySettings::default(), CancellationToken::new()),
    )
    .await
    .expect("released mapping snapshot permit")
    .expect("validation after permit release");
    drop(second);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_mapping_read_workers_hold_both_permits_until_their_work_exits() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (started_tx, started_rx) = mpsc::channel();
    let first_cancelled = CancellationToken::new();
    let second_cancelled = CancellationToken::new();

    let first_client = fixture.client();
    let first_release = Arc::clone(&release);
    let first_started = started_tx.clone();
    let first_worker_cancelled = first_cancelled.clone();
    let first = tokio::spawn(async move {
        first_client
            .run_mapping_read_for_test(first_worker_cancelled, move || {
                first_started.send(0).expect("started receiver");
                wait_until_released(&first_release);
            })
            .await
    });
    let second_client = fixture.client();
    let second_release = Arc::clone(&release);
    let second_started = started_tx.clone();
    let second_worker_cancelled = second_cancelled.clone();
    let second = tokio::spawn(async move {
        second_client
            .run_mapping_read_for_test(second_worker_cancelled, move || {
                second_started.send(1).expect("started receiver");
                wait_until_released(&second_release);
            })
            .await
    });

    let first_index = started_rx.recv().expect("first worker");
    let second_index = started_rx.recv().expect("second worker");
    assert_ne!(first_index, second_index);
    first_cancelled.cancel();
    second_cancelled.cancel();
    assert_eq!(
        first
            .await
            .expect("first mapping read task")
            .expect_err("first worker cancelled")
            .code(),
        ControlErrorCode::Cancelled
    );
    assert_eq!(
        second
            .await
            .expect("second mapping read task")
            .expect_err("second worker cancelled")
            .code(),
        ControlErrorCode::Cancelled
    );

    let turns_before_blocked_read = fixture.runtime_turn_count();
    let blocked_read = tokio::time::timeout(
        std::time::Duration::from_millis(25),
        fixture
            .client()
            .validate_mapping_settings(ProxySettings::default(), CancellationToken::new()),
    )
    .await;
    assert!(
        blocked_read.is_err(),
        "mapping read should wait for shared worker admission"
    );
    assert_eq!(
        fixture.runtime_turn_count(),
        turns_before_blocked_read,
        "mapping read must acquire bounded worker admission before fetching its runtime snapshot"
    );
    let transaction_cancelled = CancellationToken::new();
    let blocked_transaction = tokio::time::timeout(
        std::time::Duration::from_millis(25),
        fixture.client().mutate_mapping(
            create_preset("shared-admission"),
            fixture.snapshot().revision,
            SettingsTransactionOrigin::Mcp,
            transaction_cancelled.clone(),
        ),
    )
    .await;
    assert!(
        blocked_transaction.is_err(),
        "mapping transaction should share the two-worker admission bound"
    );
    assert_eq!(
        fixture.compile_count(),
        0,
        "mapping transaction must not begin blocking preparation without shared admission"
    );
    transaction_cancelled.cancel();

    let third_client = fixture.client();
    let third_release = Arc::clone(&release);
    let third = tokio::spawn(async move {
        third_client
            .run_mapping_read_for_test(CancellationToken::new(), move || {
                started_tx.send(2).expect("started receiver");
                wait_until_released(&third_release);
            })
            .await
    });
    assert!(matches!(
        started_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    {
        let (released, changed) = &*release;
        *released.lock().expect("release lock") = true;
        changed.notify_all();
    }
    assert_eq!(started_rx.recv().expect("third worker"), 2);
    third
        .await
        .expect("third mapping read task")
        .expect("third mapping read result");
}

fn wait_until_released(release: &(Mutex<bool>, Condvar)) {
    let (released, changed) = release;
    let mut released = released.lock().expect("release lock");
    while !*released {
        released = changed.wait(released).expect("release wait");
    }
}
