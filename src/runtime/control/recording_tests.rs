#![cfg(unix)]

use super::{
    RuntimeControlHandler, RuntimeGateway,
    capture_test_support::{control_context, runtime_fixture},
};
use crate::{
    control::{RuntimeReply, RuntimeRequest},
    control_rpc::{
        protocol::{ControlOperation, ControlResult, InstanceScope},
        server::ControlRpcHandler,
    },
    runtime::event_loop::execute_control_request_for_test,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

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
