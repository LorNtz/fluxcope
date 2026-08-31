#![cfg(unix)]

use std::sync::{Arc, Condvar, Mutex, mpsc};

use tokio_util::sync::CancellationToken;

use super::{
    SettingsRevision, SettingsTransactionOrigin, SettingsTransactionOutcome,
    SettingsTransactionPhase,
    test_support::{FailurePoint, SettingsRuntimeFixture, TerminalDelivery},
};
use crate::{
    control_rpc::protocol::ControlErrorCode,
    settings::{
        ConfigMode, PersistenceMode, ProxyMapRemoteRule, ProxyMapRemoteSettings,
        ProxyPresetSettings, ProxySettings,
        mapping_ops::{MappingMutation, MappingObjectRef},
    },
};

fn create_preset(name: &str) -> MappingMutation {
    MappingMutation::CreatePreset {
        name: name.to_string(),
        initial: None,
    }
}

fn complete_preset(name: &str) -> MappingMutation {
    MappingMutation::CreatePreset {
        name: name.to_string(),
        initial: Some(ProxyPresetSettings {
            name: "ignored-by-operation-name".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: false,
                rules: vec![ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://127.0.0.1:4310".to_string(),
                    enable: false,
                }],
            },
            map_local: Default::default(),
        }),
    }
}

#[tokio::test]
async fn revision_starts_at_one_and_stale_revision_changes_nothing() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    let before = fixture.snapshot();
    assert_eq!(before.revision, SettingsRevision::INITIAL);

    let committed = fixture
        .mutate(
            create_preset("one"),
            SettingsRevision::INITIAL,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect("initial revision");
    assert_eq!(committed.revision, SettingsRevision::new(2));

    let error = fixture
        .mutate(
            create_preset("stale"),
            SettingsRevision::INITIAL,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect_err("stale revision");
    assert_eq!(error.code(), ControlErrorCode::SettingsRevisionConflict);
    assert_eq!(error.details()["expected_revision"], 1);
    assert_eq!(error.details()["current_revision"], 2);
    assert_eq!(fixture.snapshot().revision, SettingsRevision::new(2));
    assert!(fixture.preset("stale").is_none());
}

#[tokio::test]
async fn serial_same_revision_writers_have_exactly_one_winner() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    let revision = fixture.snapshot().revision;
    let first = fixture.client();
    let second = fixture.client();
    let first_cancel = CancellationToken::new();
    let second_cancel = CancellationToken::new();

    let (left, right) = tokio::join!(
        first.mutate_mapping(
            create_preset("first"),
            revision,
            SettingsTransactionOrigin::Mcp,
            first_cancel,
        ),
        second.mutate_mapping(
            create_preset("second"),
            revision,
            SettingsTransactionOrigin::Mcp,
            second_cancel,
        ),
    );

    let results = [left, right];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let conflict = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one stale writer");
    assert_eq!(conflict.code(), ControlErrorCode::SettingsRevisionConflict);
    assert_eq!(fixture.snapshot().revision, SettingsRevision::new(2));
    assert_eq!(fixture.preset_names().len(), 1);
}

#[tokio::test]
async fn dirty_tui_draft_rejects_mcp_write_before_work_is_admitted() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    fixture.open_dirty_popup();
    let before = fixture.snapshot();

    let error = fixture
        .mutate(
            create_preset("blocked"),
            before.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect_err("dirty TUI conflict");

    assert_eq!(error.code(), ControlErrorCode::TuiDraftConflict);
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.worker_jobs_started(), 0);
}

#[tokio::test]
async fn admitted_transaction_blocks_tui_edits_and_saves_until_terminal_delivery() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    fixture.open_clean_popup();
    let barrier = fixture.block_next(FailurePoint::Compile);
    let revision = fixture.snapshot().revision;
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                create_preset("pending"),
                revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
    });
    barrier.entered().await;

    let edit_error = fixture
        .try_tui_edit_server_port(9196)
        .expect_err("pending edit");
    assert_eq!(edit_error.code(), ControlErrorCode::ServiceUnavailable);
    assert_eq!(
        edit_error.details()["stage"],
        "settings_transaction_pending"
    );
    let save_error = fixture.try_tui_save().expect_err("pending save");
    assert_eq!(save_error.code(), ControlErrorCode::ServiceUnavailable);
    assert_eq!(
        save_error.details()["stage"],
        "settings_transaction_pending"
    );
    assert!(fixture.popup_is_visible());

    barrier.release();
    task.await
        .expect("transaction task")
        .expect("transaction completes");
    fixture
        .try_tui_edit_server_port(9196)
        .expect("token cleared");
}

#[tokio::test]
async fn domain_validation_failure_is_atomic_and_reports_validation_stage() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    let before = fixture.snapshot();
    let invalid = MappingMutation::AppendRemoteRule {
        preset: "missing".to_string(),
        rule: ProxyMapRemoteRule::default(),
    };

    let error = fixture
        .mutate(
            invalid,
            before.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect_err("validation failure");

    assert_eq!(error.code(), ControlErrorCode::InvalidArgument);
    assert_eq!(error.details()["stage"], "validation");
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn policy_compilation_failure_changes_no_authoritative_state() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    fixture.fail_next(FailurePoint::Compile);
    let before = fixture.snapshot();

    let error = fixture
        .mutate(
            create_preset("compile-fails"),
            before.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect_err("compile failure");

    assert_eq!(error.code(), ControlErrorCode::InvalidArgument);
    assert_eq!(error.details()["stage"], "compilation");
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.terminal_deliveries(), vec![TerminalDelivery::Abort]);
}

#[tokio::test]
async fn failed_write_or_fsync_changes_nothing_and_aborts_once() {
    for point in [FailurePoint::WriteTemporary, FailurePoint::FsyncTemporary] {
        let fixture = SettingsRuntimeFixture::persistent().await;
        fixture.fail_next(point);
        let before = fixture.snapshot();

        let error = fixture
            .mutate(
                create_preset("io-fails"),
                before.revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
            .expect_err("persistence failure");

        assert_eq!(error.code(), ControlErrorCode::InternalError);
        assert_eq!(error.details()["stage"], "persistence");
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(fixture.persisted_bytes(), before.persisted_bytes);
        assert_eq!(fixture.terminal_deliveries(), vec![TerminalDelivery::Abort]);
    }
}

#[tokio::test]
async fn rename_failure_from_committing_preserves_live_and_file_state_and_aborts_once() {
    let fixture = SettingsRuntimeFixture::persistent().await;
    fixture.fail_next(FailurePoint::Rename);
    let before = fixture.snapshot();

    let error = fixture
        .mutate(
            create_preset("rename-fails"),
            before.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect_err("rename failure");

    assert_eq!(error.code(), ControlErrorCode::InternalError);
    assert_eq!(error.details()["stage"], "persistence");
    assert_eq!(error.details()["operation"], "rename");
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.persisted_bytes(), before.persisted_bytes);
    assert_eq!(fixture.terminal_deliveries(), vec![TerminalDelivery::Abort]);
}

#[tokio::test]
async fn persistent_success_reports_persistent_metadata_and_updates_file_and_policy() {
    let fixture = SettingsRuntimeFixture::persistent().await;
    let before = fixture.snapshot();

    let result = fixture
        .mutate(
            complete_preset("dev"),
            before.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect("persistent commit");

    assert_eq!(result.outcome, SettingsTransactionOutcome::Committed);
    assert_eq!(result.revision, SettingsRevision::new(2));
    assert_eq!(result.config_mode, ConfigMode::DefaultOwned);
    assert_eq!(result.persistence, PersistenceMode::Persistent);
    assert_eq!(
        result.affected,
        MappingObjectRef::Preset {
            name: "dev".to_string()
        }
    );
    assert_ne!(fixture.persisted_bytes(), before.persisted_bytes);
    assert!(
        !fixture.persisted_settings().proxy.unwrap().presets[0]
            .map_remote
            .rules[0]
            .enable
    );
    assert_eq!(
        fixture.terminal_deliveries(),
        vec![TerminalDelivery::Finalize]
    );
}

#[tokio::test]
async fn read_only_and_temporary_success_are_ephemeral_and_never_write() {
    for fixture in [
        SettingsRuntimeFixture::read_only().await,
        SettingsRuntimeFixture::temporary().await,
    ] {
        let before = fixture.snapshot();
        let result = fixture
            .mutate(
                create_preset("ephemeral"),
                before.revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
            .expect("ephemeral commit");

        assert_eq!(result.outcome, SettingsTransactionOutcome::Committed);
        assert_eq!(result.persistence, PersistenceMode::Ephemeral);
        assert!(matches!(
            result.config_mode,
            ConfigMode::ReadOnlyFile | ConfigMode::Temporary
        ));
        assert_eq!(fixture.persisted_bytes(), before.persisted_bytes);
        assert_eq!(fixture.external_write_count(), 0);
        assert_eq!(fixture.snapshot().revision, SettingsRevision::new(2));
    }
}

#[tokio::test]
async fn unchanged_mutation_skips_compile_persistence_policy_swap_and_revision_advance() {
    let fixture = SettingsRuntimeFixture::persistent_with_preset("dev").await;
    let before = fixture.snapshot();

    let result = fixture
        .mutate(
            MappingMutation::SetActivePreset {
                name: Some("dev".to_string()),
            },
            before.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect("unchanged result");

    assert_eq!(result.outcome, SettingsTransactionOutcome::Unchanged);
    assert_eq!(result.revision, before.revision);
    assert_eq!(result.persistence, PersistenceMode::Persistent);
    assert_eq!(result.affected, MappingObjectRef::ActivePreset);
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.compile_count(), 0);
    assert_eq!(fixture.external_write_count(), 0);
    assert_eq!(fixture.policy_swap_count(), 0);
    assert_eq!(
        fixture.terminal_deliveries(),
        vec![TerminalDelivery::Finalize]
    );
}

#[tokio::test]
async fn complete_initial_preset_is_preserved_and_never_activated_implicitly() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    let revision = fixture.snapshot().revision;

    fixture
        .mutate(
            complete_preset("dev"),
            revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .expect("complete preset");

    let proxy = fixture
        .snapshot()
        .settings
        .proxy
        .clone()
        .expect("proxy created");
    assert_eq!(proxy.active_preset, None);
    assert_eq!(proxy.presets[0].name, "dev");
    assert!(!proxy.presets[0].map_remote.enable);
    assert_eq!(proxy.presets[0].map_remote.rules.len(), 1);
    assert!(!proxy.presets[0].map_remote.rules[0].enable);
}

#[tokio::test]
async fn active_preset_commit_swaps_precompiled_policy_before_publishing_new_settings() {
    let fixture = SettingsRuntimeFixture::temporary_with_remote_presets().await;
    let barrier = fixture.block_next(FailurePoint::FinalizeSettingsSwap);
    let revision = fixture.snapshot().revision;
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                MappingMutation::SetActivePreset {
                    name: Some("second".to_string()),
                },
                revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
    });
    barrier.entered().await;

    assert_eq!(fixture.active_policy_preset().as_deref(), Some("second"));
    assert_eq!(fixture.snapshot().active_preset().as_deref(), Some("first"));

    barrier.release();
    task.await.expect("transaction task").expect("commit");
    assert_eq!(
        fixture.snapshot().active_preset().as_deref(),
        Some("second")
    );
    assert_eq!(
        fixture.terminal_deliveries(),
        vec![TerminalDelivery::Finalize]
    );
}

#[tokio::test]
async fn clean_open_popup_refreshes_only_after_successful_finalization() {
    let fixture = SettingsRuntimeFixture::temporary().await;
    fixture.open_clean_popup();
    let barrier = fixture.block_next(FailurePoint::Finalize);
    let revision = fixture.snapshot().revision;
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                create_preset("popup"),
                revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
    });
    barrier.entered().await;

    assert!(fixture.popup_preset("popup").is_none());
    barrier.release();
    task.await.expect("transaction task").expect("commit");
    assert!(fixture.popup_is_visible());
    assert!(!fixture.popup_is_dirty());
    assert!(fixture.popup_preset("popup").is_some());
}

#[tokio::test]
async fn cancellation_during_temp_write_or_fsync_aborts_before_rename() {
    for point in [FailurePoint::WriteTemporary, FailurePoint::FsyncTemporary] {
        let fixture = SettingsRuntimeFixture::persistent().await;
        let barrier = fixture.block_next(point);
        let before = fixture.snapshot();
        let cancellation = CancellationToken::new();
        let child = cancellation.clone();
        let client = fixture.client();
        let task = tokio::spawn(async move {
            client
                .mutate_mapping(
                    create_preset("cancelled"),
                    before.revision,
                    SettingsTransactionOrigin::Mcp,
                    child,
                )
                .await
        });
        barrier.entered().await;
        cancellation.cancel();
        barrier.release();

        let error = task
            .await
            .expect("transaction task")
            .expect_err("cancelled before commit");
        assert_eq!(error.code(), ControlErrorCode::Cancelled);
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(fixture.persisted_bytes(), before.persisted_bytes);
        assert_eq!(fixture.terminal_deliveries(), vec![TerminalDelivery::Abort]);
    }
}

#[tokio::test]
async fn cancellation_at_precommit_barrier_aborts_and_clears_pending_token_once() {
    let fixture = SettingsRuntimeFixture::persistent().await;
    let barrier = fixture.block_next(FailurePoint::PreCommit);
    let before = fixture.snapshot();
    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                create_preset("cancelled"),
                before.revision,
                SettingsTransactionOrigin::Mcp,
                child,
            )
            .await
    });
    barrier.entered().await;
    assert_eq!(
        fixture.transaction_phase(),
        Some(SettingsTransactionPhase::PreCommit)
    );
    cancellation.cancel();
    barrier.release();

    assert_eq!(
        task.await
            .expect("transaction task")
            .expect_err("cancelled")
            .code(),
        ControlErrorCode::Cancelled
    );
    assert_eq!(fixture.snapshot(), before);
    assert!(!fixture.has_pending_transaction());
    assert_eq!(fixture.terminal_deliveries(), vec![TerminalDelivery::Abort]);
}

#[tokio::test]
async fn cancellation_after_committing_transition_cannot_interrupt_commit() {
    let fixture = SettingsRuntimeFixture::persistent().await;
    let barrier = fixture.block_next(FailurePoint::Rename);
    let revision = fixture.snapshot().revision;
    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                create_preset("committing"),
                revision,
                SettingsTransactionOrigin::Mcp,
                child,
            )
            .await
    });
    barrier.entered().await;
    assert_eq!(
        fixture.transaction_phase(),
        Some(SettingsTransactionPhase::Committing)
    );
    cancellation.cancel();
    barrier.release();

    let result = task.await.expect("transaction task").expect("must commit");
    assert_eq!(result.outcome, SettingsTransactionOutcome::Committed);
    assert_eq!(result.revision, SettingsRevision::new(2));
    assert!(fixture.preset("committing").is_some());
    assert_eq!(
        fixture.terminal_deliveries(),
        vec![TerminalDelivery::Finalize]
    );
}

#[tokio::test]
async fn cancellation_after_committed_transition_still_delivers_finalize() {
    let fixture = SettingsRuntimeFixture::persistent().await;
    let barrier = fixture.block_next(FailurePoint::Committed);
    let revision = fixture.snapshot().revision;
    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                create_preset("committed"),
                revision,
                SettingsTransactionOrigin::Mcp,
                child,
            )
            .await
    });
    barrier.entered().await;
    assert_eq!(
        fixture.transaction_phase(),
        Some(SettingsTransactionPhase::Committed)
    );
    cancellation.cancel();
    barrier.release();

    let result = task.await.expect("transaction task").expect("finalized");
    assert_eq!(result.revision, SettingsRevision::new(2));
    assert_eq!(
        fixture.terminal_deliveries(),
        vec![TerminalDelivery::Finalize]
    );
    assert!(!fixture.has_pending_transaction());
}

#[tokio::test]
async fn changed_transaction_reserves_terminal_delivery_before_commit() {
    let fixture = SettingsRuntimeFixture::persistent().await;
    let barrier = fixture.block_next(FailurePoint::Committed);
    let revision = fixture.snapshot().revision;
    let client = fixture.client();
    let task = tokio::spawn(async move {
        client
            .mutate_mapping(
                create_preset("reserved"),
                revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
    });

    barrier.entered().await;
    assert_eq!(fixture.available_terminal_delivery_permits(), 0);
    barrier.release();

    task.await
        .expect("transaction task")
        .expect("transaction finalizes");
}

#[tokio::test]
async fn shutdown_aborts_precommit_but_drains_committing_and_committed_transactions() {
    for point in [
        FailurePoint::PreCommit,
        FailurePoint::Rename,
        FailurePoint::Committed,
    ] {
        let fixture = Arc::new(SettingsRuntimeFixture::persistent().await);
        let barrier = fixture.block_next(point);
        let revision = fixture.snapshot().revision;
        let client = fixture.client();
        let task = tokio::spawn(async move {
            client
                .mutate_mapping(
                    create_preset("shutdown"),
                    revision,
                    SettingsTransactionOrigin::Mcp,
                    CancellationToken::new(),
                )
                .await
        });
        barrier.entered().await;
        let shutdown_fixture = Arc::clone(&fixture);
        let shutdown = tokio::spawn(async move { shutdown_fixture.shutdown().await });
        fixture.shutdown_started().await;
        assert!(fixture.gateway_accepts_reserved_terminal_delivery());
        barrier.release();
        shutdown.await.expect("shutdown task");

        let result = task.await.expect("transaction task");
        if point == FailurePoint::PreCommit {
            assert_eq!(
                result.expect_err("precommit cancelled").code(),
                ControlErrorCode::Cancelled
            );
            assert_eq!(fixture.terminal_deliveries(), vec![TerminalDelivery::Abort]);
        } else {
            assert_eq!(
                result.expect("commit drains").revision,
                SettingsRevision::new(2)
            );
            assert_eq!(
                fixture.terminal_deliveries(),
                vec![TerminalDelivery::Finalize]
            );
        }
        assert!(!fixture.has_pending_transaction());
        assert!(fixture.gateway_is_closed());
    }
}

#[tokio::test]
async fn slow_compile_and_persistence_never_block_runtime_event_loop() {
    for point in [FailurePoint::Compile, FailurePoint::FsyncTemporary] {
        let fixture = SettingsRuntimeFixture::persistent().await;
        let barrier = fixture.block_next(point);
        let revision = fixture.snapshot().revision;
        let client = fixture.client();
        let task = tokio::spawn(async move {
            client
                .mutate_mapping(
                    create_preset("slow"),
                    revision,
                    SettingsTransactionOrigin::Mcp,
                    CancellationToken::new(),
                )
                .await
        });
        barrier.entered().await;

        let before = fixture.runtime_turn_count();
        fixture.ping_runtime().await.expect("runtime ping");
        assert!(fixture.runtime_turn_count() > before);

        barrier.release();
        task.await.expect("transaction task").expect("transaction");
    }
}

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
    let proxy = read.settings.proxy.clone().unwrap_or_default();

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
        .get_mapping_settings(CancellationToken::new())
        .await
        .expect("first mapping snapshot");
    let second = fixture
        .client()
        .get_mapping_settings(CancellationToken::new())
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

#[test]
fn settings_transaction_service_is_bounded_serial_and_supervised_as_fatal() {
    assert_eq!(super::SETTINGS_TRANSACTION_QUEUE_CAPACITY, 8);
    assert!(super::super::services::ServiceKind::SettingsTransactions.is_fatal());
}
